//! Bounded P5.3 event-order certificates for vector loop fission.
//!
//! # C++ provenance and formal boundary
//! Faust C++ performs loop fission after signal dependencies and recursive
//! state have constrained the loop DAG (`DAGInstructionsCompiler` and
//! `CodeLoop`). The port plan states the corresponding proof obligation as
//! `FissionSafe`: every dynamic dependence ordered by scalar execution must
//! remain ordered by vector execution.
//!
//! This module makes that obligation executable for routed plans. While the
//! complete event table fits the explicit bound, it expands each loop operation
//! over the vector chunk. Larger sample-repetitive plans use a canonical
//! two-sample basis that checks one complete body and every adjacent carried
//! boundary. Both forms build a sample-major scalar order, a scheduled
//! loop-major vector order, and a conservative dependence relation. Conflicting
//! effect events are ordered as they are in the scalar reference. Consequently,
//! cross-loop carried state is rejected even when a static effect edge happens
//! to order the two loops.
//!
//! The model is deliberately bounded. Its base form is the structural P5 gate;
//! its state-refined form consumes P6.1 `DelaySim`/`RecStep` evidence and
//! replaces the corresponding conservative effects with explicit
//! `LoopPre`/sample/`LoopPost` events. Neither form proves complete DSP
//! semantics. Production construction and independent checking require an
//! explicit event limit and fail closed when neither the complete chunk nor the
//! independently reconstructed two-sample basis fits it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[cfg(test)]
use super::vector_analysis::effects_conflict;
use super::vector_analysis::{EffectAtom, ForeignPurity, StateResource};
use super::vector_plan::VerifiedVectorPlan;
use super::vector_route::{RoutedUseSource, VectorRegion, VerifiedRoutedFir};
use super::vector_state::{VectorStateAction, VerifiedVectorStatePlan};
use super::vector_verify::{LoopEdge, VectorPlan, VectorPlanError, verify_vector_plan};

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
    complete: usize,
    compact: usize,
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
const COMPACT_EVENT_SAMPLE_BASIS: usize = 2;

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
    /// A named P4.4 chunk transport.
    Transport(u64),
}

/// One operation represented by the bounded event model.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VectorEventKind {
    /// A routed signal definition.
    Definition { signal_id: u64 },
    /// One exact signal-level effect atom attached to a definition.
    Effect {
        signal_id: u64,
        effect_index: u64,
        effect: EffectAtom,
    },
    /// Producer-side write into a named chunk transport.
    TransportStore { transport_id: u64 },
    /// Consumer-side read from a named chunk transport.
    TransportLoad { transport_id: u64 },
    /// One routed signal use. `occurrence` distinguishes repeated equal uses.
    Use {
        signal_id: u64,
        source: EventUseSource,
        occurrence: u64,
    },
    /// Fixed entry barrier for one epoch.
    EpochEnter { epoch_id: u64 },
    /// Fixed exit barrier for one epoch.
    EpochExit { epoch_id: u64 },
    /// One checked P6.1 delay or recursion transition.
    StateTransition { action: VectorStateAction },
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
    sample_count: u64,
    /// Number of concrete samples retained in the finite event table. This is
    /// equal to `sample_count` for the expanded model and two for a repeated
    /// induction basis that checks both one sample and the `n -> n + 1`
    /// carried-state boundary.
    checked_sample_count: u64,
    events: Vec<VectorEvent>,
    scalar_order: Vec<u64>,
    vector_order: Vec<u64>,
    dependencies: Vec<EventDependency>,
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

/// Opaque evidence that [`verify_event_order_certificate`] accepted P5.3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedEventOrderCertificate {
    certificate: EventOrderCertificate,
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

/// Typed producer/checker failure for the P5.3 event-order gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VectorEventError {
    /// The P4.4 plan is not independently valid.
    Plan(VectorPlanError),
    /// The routed artifact was produced from a different plan.
    RoutedPlanMismatch,
    /// The P6.1 state artifact was produced from a different vector plan.
    StatePlanMismatch,
    /// The route layout does not exactly and topologically cover the plan.
    InvalidLayout { detail: &'static str, loop_id: u64 },
    /// The selected complete or compact finite table is larger than its bound.
    EventBoundExceeded { needed: usize, limit: usize },
    /// A route-independent lower bound already exceeds both applicable bounds.
    EventLowerBoundExceeded { minimum: usize, limit: usize },
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
    InvalidOrder { which: &'static str },
    /// A reconstructed dependence contradicts scalar execution.
    ScalarDependenceViolation { before: u64, after: u64 },
    /// A scalar-ordered dependence is reversed by vector execution.
    FissionSafeViolation { before: u64, after: u64 },
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

/// Rejects plans whose route-independent event lower bound already exceeds
/// the applicable complete-expansion or compact-basis limit.
///
/// This is intentionally one-sided: it counts only non-tuple loop roots,
/// their unmanaged effects, planned transport store/load pairs, epoch
/// barriers, and exact state actions. Routed inline definitions and uses are
/// omitted, so passing this precheck is not certification. Rejection is safe
/// because the complete event table must contain every counted event.
pub fn precheck_state_event_bound(
    verified_plan: &VerifiedVectorPlan,
    state: &VerifiedVectorStatePlan,
    limits: EventLimits,
) -> Result<(), VectorEventError> {
    let plan = verified_plan.plan();
    if state.vector_plan() != plan {
        return Err(VectorEventError::StatePlanMismatch);
    }
    let managed = state.managed_resources();
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let mut per_sample = plan
        .transports
        .len()
        .checked_mul(2)
        .ok_or(VectorEventError::EventCountOverflow)?;
    for record in &plan.loops {
        for root in &record.roots {
            let signal = signals
                .get(root)
                .ok_or(VectorEventError::EventCountOverflow)?;
            if matches!(signal.value_type, super::vector_verify::ValueType::Tuple(_)) {
                continue;
            }
            per_sample = per_sample
                .checked_add(1)
                .and_then(|count| {
                    // Emission now attributes an effect to its performer, so a
                    // root contributes only its direct set; counting the
                    // transitive union here would exceed actual emission and
                    // turn this lower bound into a spurious rejection.
                    count.checked_add(
                        signal
                            .direct_effects
                            .iter()
                            .filter(|effect| !effect_is_managed(effect, &managed))
                            .count(),
                    )
                })
                .ok_or(VectorEventError::EventCountOverflow)?;
        }
    }
    let mut fixed = plan
        .epochs
        .len()
        .checked_mul(2)
        .ok_or(VectorEventError::EventCountOverflow)?;
    for phases in &state.plan().loops {
        per_sample = per_sample
            .checked_add(phases.exec.len())
            .ok_or(VectorEventError::EventCountOverflow)?;
        fixed = fixed
            .checked_add(phases.pre.len())
            .and_then(|count| count.checked_add(phases.post.len()))
            .ok_or(VectorEventError::EventCountOverflow)?;
    }
    let sample_count =
        usize::try_from(plan.vec_size).map_err(|_| VectorEventError::EventCountOverflow)?;
    let minimum = per_sample
        .checked_mul(sample_count)
        .and_then(|count| count.checked_add(fixed))
        .ok_or(VectorEventError::EventCountOverflow)?;
    if minimum <= limits.complete {
        return Ok(());
    }
    let checked_samples = sample_count.min(COMPACT_EVENT_SAMPLE_BASIS);
    let compact_minimum = per_sample
        .checked_mul(checked_samples)
        .and_then(|count| count.checked_add(fixed))
        .ok_or(VectorEventError::EventCountOverflow)?;
    if compact_minimum > limits.compact {
        return Err(VectorEventError::EventLowerBoundExceeded {
            minimum: compact_minimum,
            limit: limits.compact,
        });
    }
    Ok(())
}

/// Produces and independently checks bounded P5.3 evidence.
///
/// `limits.complete` applies to full `vec_size` expansion and `limits.compact`
/// applies only to the canonical two-sample basis. Neither form changes the
/// logical chunk length.
pub fn build_event_order_certificate(
    verified_plan: &VerifiedVectorPlan,
    routed: &VerifiedRoutedFir,
    limits: EventLimits,
) -> Result<VerifiedEventOrderCertificate, VectorEventError> {
    let plan = verified_plan.plan();
    let certificate = derive_certificate(plan, routed, None, limits)?;
    verify_event_order_certificate(plan, routed, &certificate, limits)?;
    Ok(VerifiedEventOrderCertificate { certificate })
}

/// Produces P5.3 evidence refined by checked P6.1 state transitions.
pub fn build_state_event_order_certificate(
    verified_plan: &VerifiedVectorPlan,
    routed: &VerifiedRoutedFir,
    state: &VerifiedVectorStatePlan,
    limits: EventLimits,
) -> Result<VerifiedEventOrderCertificate, VectorEventError> {
    let plan = verified_plan.plan();
    if state.vector_plan() != plan {
        return Err(VectorEventError::StatePlanMismatch);
    }
    let certificate = derive_certificate(plan, routed, Some(state), limits)?;
    verify_state_event_order_certificate(plan, routed, state, &certificate, limits)?;
    Ok(VerifiedEventOrderCertificate { certificate })
}

/// Independently reconstructs and checks event coverage, both total orders,
/// the dependence relation, and `FissionSafe`.
pub fn verify_event_order_certificate(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    certificate: &EventOrderCertificate,
    limits: EventLimits,
) -> Result<(), VectorEventError> {
    verify_event_order_certificate_impl(plan, routed, None, certificate, limits)
}

/// Independently checks a state-refined P5.3/P6.1 event certificate.
pub fn verify_state_event_order_certificate(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: &VerifiedVectorStatePlan,
    certificate: &EventOrderCertificate,
    limits: EventLimits,
) -> Result<(), VectorEventError> {
    if state.vector_plan() != plan {
        return Err(VectorEventError::StatePlanMismatch);
    }
    verify_event_order_certificate_impl(plan, routed, Some(state), certificate, limits)
}

fn verify_event_order_certificate_impl(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: Option<&VerifiedVectorStatePlan>,
    certificate: &EventOrderCertificate,
    limits: EventLimits,
) -> Result<(), VectorEventError> {
    verify_vector_plan(plan)?;
    if routed.plan() != plan {
        return Err(VectorEventError::RoutedPlanMismatch);
    }
    validate_layout(plan, routed)?;
    if certificate.sample_count != plan.vec_size {
        return Err(VectorEventError::EventTableMismatch);
    }
    let expected_checked = independent_checked_sample_count(plan, routed, state, limits)?;
    if certificate.checked_sample_count != expected_checked {
        return Err(VectorEventError::CompactRepetitionMismatch);
    }
    let mut checked_plan = plan.clone();
    checked_plan.vec_size = expected_checked;
    let finite_limit = if expected_checked == plan.vec_size {
        limits.complete
    } else {
        limits.compact
    };
    verify_event_table_independently(
        &checked_plan,
        routed,
        state,
        &certificate.events,
        finite_limit,
    )?;
    if expected_checked < plan.vec_size {
        verify_compact_repetition_basis(&checked_plan, &certificate.events)?;
    }
    validate_order("scalar", &certificate.events, &certificate.scalar_order)?;
    validate_order("vector", &certificate.events, &certificate.vector_order)?;
    let scalar_order = independently_order_events(&checked_plan, routed, &certificate.events, true);
    if certificate.scalar_order != scalar_order {
        return Err(VectorEventError::ScalarOrderMismatch);
    }
    let vector_order =
        independently_order_events(&checked_plan, routed, &certificate.events, false);
    if certificate.vector_order != vector_order {
        return Err(VectorEventError::VectorOrderMismatch);
    }
    if certificate
        .dependencies
        .windows(2)
        .any(|pair| pair[0] >= pair[1])
    {
        return Err(VectorEventError::DependencyMismatch);
    }

    let scalar_positions = positions(&certificate.scalar_order);
    let vector_positions = positions(&certificate.vector_order);
    let event_ids = certificate
        .events
        .iter()
        .map(|event| event.event_id)
        .collect::<BTreeSet<_>>();
    for dependency in &certificate.dependencies {
        if dependency.before == dependency.after
            || !event_ids.contains(&dependency.before)
            || !event_ids.contains(&dependency.after)
        {
            return Err(VectorEventError::DependencyMismatch);
        }
        if scalar_positions[&dependency.before] >= scalar_positions[&dependency.after] {
            return Err(VectorEventError::ScalarDependenceViolation {
                before: dependency.before,
                after: dependency.after,
            });
        }
        if vector_positions[&dependency.before] >= vector_positions[&dependency.after] {
            if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                let by_id = certificate
                    .events
                    .iter()
                    .map(|event| (event.event_id, event))
                    .collect::<BTreeMap<_, _>>();
                eprintln!(
                    "[vector-event-reversal] before={:?} scalar={} vector={} after={:?} scalar={} vector={} fused={:?}",
                    by_id[&dependency.before],
                    scalar_positions[&dependency.before],
                    vector_positions[&dependency.before],
                    by_id[&dependency.after],
                    scalar_positions[&dependency.after],
                    vector_positions[&dependency.after],
                    checked_plan.fused_serial_groups
                );
            }
            return Err(VectorEventError::FissionSafeViolation {
                before: dependency.before,
                after: dependency.after,
            });
        }
    }
    verify_required_dependencies(
        &checked_plan,
        state,
        &certificate.events,
        &certificate.dependencies,
        &scalar_positions,
    )?;
    Ok(())
}

fn independent_checked_sample_count(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: Option<&VerifiedVectorStatePlan>,
    limits: EventLimits,
) -> Result<u64, VectorEventError> {
    let mut one_sample_plan = plan.clone();
    one_sample_plan.vec_size = 1;
    let keys = independently_expected_event_keys(&one_sample_plan, routed, state)?;
    let fixed = keys
        .iter()
        .filter(|(_, sample, _)| sample.is_none())
        .count();
    let per_sample = keys
        .iter()
        .filter(|(_, sample, _)| sample.is_some())
        .count();
    let logical_samples =
        usize::try_from(plan.vec_size).map_err(|_| VectorEventError::EventCountOverflow)?;
    let complete = per_sample
        .checked_mul(logical_samples)
        .and_then(|count| count.checked_add(fixed))
        .ok_or(VectorEventError::EventCountOverflow)?;
    if complete <= limits.complete {
        return Ok(plan.vec_size);
    }
    let basis = logical_samples.min(COMPACT_EVENT_SAMPLE_BASIS);
    let compact = per_sample
        .checked_mul(basis)
        .and_then(|count| count.checked_add(fixed))
        .ok_or(VectorEventError::EventCountOverflow)?;
    if compact > limits.compact {
        return Err(VectorEventError::EventBoundExceeded {
            needed: compact,
            limit: limits.compact,
        });
    }
    u64::try_from(basis).map_err(|_| VectorEventError::EventCountOverflow)
}

fn verify_compact_repetition_basis(
    checked_plan: &VectorPlan,
    events: &[VectorEvent],
) -> Result<(), VectorEventError> {
    if checked_plan.vec_size != COMPACT_EVENT_SAMPLE_BASIS as u64 {
        return Err(VectorEventError::CompactRepetitionMismatch);
    }
    for record in &checked_plan.loops {
        let sample_zero = events
            .iter()
            .filter(|event| {
                event.region == EventRegion::Loop(record.loop_id) && event.sample == Some(0)
            })
            .map(|event| &event.kind)
            .collect::<Vec<_>>();
        let sample_one = events
            .iter()
            .filter(|event| {
                event.region == EventRegion::Loop(record.loop_id) && event.sample == Some(1)
            })
            .map(|event| &event.kind)
            .collect::<Vec<_>>();
        if sample_zero != sample_one {
            return Err(VectorEventError::CompactRepetitionMismatch);
        }
    }
    Ok(())
}

type EventKey = (EventRegion, Option<u64>, VectorEventKind);

fn append_state_event_keys(
    keys: &mut Vec<EventKey>,
    state: &VerifiedVectorStatePlan,
    sample_count: u64,
) {
    for phases in &state.plan().loops {
        for action in &phases.pre {
            keys.push((
                EventRegion::LoopPre(phases.loop_id),
                None,
                VectorEventKind::StateTransition {
                    action: action.clone(),
                },
            ));
        }
        for sample in 0..sample_count {
            for action in &phases.exec {
                keys.push((
                    EventRegion::Loop(phases.loop_id),
                    Some(sample),
                    VectorEventKind::StateTransition {
                        action: action.clone(),
                    },
                ));
            }
        }
        for action in &phases.post {
            keys.push((
                EventRegion::LoopPost(phases.loop_id),
                None,
                VectorEventKind::StateTransition {
                    action: action.clone(),
                },
            ));
        }
    }
}

fn effect_is_managed(effect: &EffectAtom, managed: &BTreeSet<StateResource>) -> bool {
    match effect {
        EffectAtom::ReadState(resource) | EffectAtom::WriteState(resource) => {
            managed.contains(resource)
        }
        _ => false,
    }
}

fn verify_event_table_independently(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: Option<&VerifiedVectorStatePlan>,
    events: &[VectorEvent],
    event_limit: usize,
) -> Result<(), VectorEventError> {
    let expected = independently_expected_event_keys(plan, routed, state)?;
    if expected.len() > event_limit {
        return Err(VectorEventError::EventBoundExceeded {
            needed: expected.len(),
            limit: event_limit,
        });
    }
    if events.len() != expected.len() {
        return Err(VectorEventError::EventTableMismatch);
    }
    for (index, (event, key)) in events.iter().zip(expected).enumerate() {
        let event_id = u64::try_from(index).map_err(|_| VectorEventError::EventCountOverflow)?;
        if event.event_id != event_id || (event.region, event.sample, event.kind.clone()) != key {
            return Err(VectorEventError::EventTableMismatch);
        }
    }
    Ok(())
}

fn independently_expected_event_keys(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: Option<&VerifiedVectorStatePlan>,
) -> Result<Vec<EventKey>, VectorEventError> {
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let transports = plan
        .transports
        .iter()
        .map(|transport| (transport.transport_id, transport))
        .collect::<BTreeMap<_, _>>();
    let managed = state.map_or_else(BTreeSet::new, VerifiedVectorStatePlan::managed_resources);
    let mut loop_kinds = BTreeMap::<u64, Vec<VectorEventKind>>::new();
    let mut keys = Vec::new();

    for definition in routed.trace().definitions() {
        let kinds = match definition.region {
            VectorRegion::Control => None,
            VectorRegion::Loop(loop_id) => Some(loop_kinds.entry(loop_id).or_default()),
        };
        let definition_kind = VectorEventKind::Definition {
            signal_id: definition.signal_id,
        };
        if let Some(kinds) = kinds {
            kinds.push(definition_kind);
        } else {
            keys.push((EventRegion::Control, None, definition_kind));
        }
        // An effect event models one actual operation, so it belongs to the
        // signal that performs it. Enumerating the transitive closure instead
        // emits one event per carrier and invents conflicts between signals
        // that merely contain the performer in their subtree.
        for (effect_index, effect) in signals[&definition.signal_id]
            .direct_effects
            .iter()
            .enumerate()
        {
            if effect_is_managed(effect, &managed) {
                continue;
            }
            let kind = VectorEventKind::Effect {
                signal_id: definition.signal_id,
                effect_index: u64::try_from(effect_index)
                    .map_err(|_| VectorEventError::EventCountOverflow)?,
                effect: effect.clone(),
            };
            match definition.region {
                VectorRegion::Control => keys.push((EventRegion::Control, None, kind)),
                VectorRegion::Loop(loop_id) => loop_kinds.entry(loop_id).or_default().push(kind),
            }
        }
    }
    for routed_transport in routed.trace().transports() {
        let transport = transports[&routed_transport.transport_id];
        loop_kinds.entry(transport.producer_loop).or_default().push(
            VectorEventKind::TransportStore {
                transport_id: transport.transport_id,
            },
        );
        loop_kinds.entry(transport.consumer_loop).or_default().push(
            VectorEventKind::TransportLoad {
                transport_id: transport.transport_id,
            },
        );
    }
    let mut uses = routed
        .trace()
        .uses()
        .iter()
        .map(|routed_use| {
            (
                routed_use.consumer_loop,
                routed_use.signal_id,
                event_use_source(routed_use.source),
            )
        })
        .collect::<Vec<_>>();
    uses.sort_unstable();
    let mut occurrences = BTreeMap::new();
    for (consumer_loop, signal_id, source) in uses {
        let occurrence = occurrences
            .entry((consumer_loop, signal_id, source))
            .or_insert(0_u64);
        loop_kinds
            .entry(consumer_loop)
            .or_default()
            .push(VectorEventKind::Use {
                signal_id,
                source,
                occurrence: *occurrence,
            });
        *occurrence = occurrence
            .checked_add(1)
            .ok_or(VectorEventError::EventCountOverflow)?;
    }
    for epoch in &plan.epochs {
        keys.push((
            EventRegion::Epoch(epoch.epoch_id),
            None,
            VectorEventKind::EpochEnter {
                epoch_id: epoch.epoch_id,
            },
        ));
        keys.push((
            EventRegion::Epoch(epoch.epoch_id),
            None,
            VectorEventKind::EpochExit {
                epoch_id: epoch.epoch_id,
            },
        ));
    }
    for record in &plan.loops {
        let kinds = loop_kinds.remove(&record.loop_id).unwrap_or_default();
        for sample in 0..plan.vec_size {
            for kind in &kinds {
                keys.push((
                    EventRegion::Loop(record.loop_id),
                    Some(sample),
                    kind.clone(),
                ));
            }
        }
    }
    if let Some(state) = state {
        append_state_event_keys(&mut keys, state, plan.vec_size);
    }
    keys.sort();
    Ok(keys)
}

fn independently_order_events(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    events: &[VectorEvent],
    sample_major: bool,
) -> Vec<u64> {
    let epochs = sorted_epochs(plan);
    let epoch_position = epochs
        .iter()
        .enumerate()
        .map(|(position, epoch)| (epoch.epoch_id, position + 1))
        .collect::<BTreeMap<_, _>>();
    let loop_epoch = plan
        .loops
        .iter()
        .map(|record| (record.loop_id, record.epoch_id))
        .collect::<BTreeMap<_, _>>();
    let canonical_loops = canonical_scalar_loops(plan);
    let scalar_loop_position = canonical_loops
        .values()
        .flat_map(|loops| loops.iter().enumerate())
        .map(|(position, loop_id)| (*loop_id, position))
        .collect::<BTreeMap<_, _>>();
    let vector_loop_position = routed
        .layout()
        .loops()
        .iter()
        .enumerate()
        .map(|(position, region)| (region.loop_id, position))
        .collect::<BTreeMap<_, _>>();
    let mut sample_interleaved_position = BTreeMap::new();
    for group in &plan.fused_serial_groups {
        let members = routed
            .layout()
            .loops()
            .iter()
            .filter(|region| group.member_loop_ids.binary_search(&region.loop_id).is_ok())
            .map(|region| region.loop_id)
            .collect::<Vec<_>>();
        let unit_position = members
            .iter()
            .map(|loop_id| vector_loop_position[loop_id])
            .min()
            .expect("verified fused groups are non-empty");
        for (member_position, loop_id) in members.into_iter().enumerate() {
            sample_interleaved_position.insert(loop_id, (unit_position, member_position));
        }
    }
    for bundle in &plan.lockstep_bundles {
        let unit_position = bundle
            .member_loop_ids
            .iter()
            .map(|loop_id| vector_loop_position[loop_id])
            .min()
            .expect("verified lockstep bundles are non-empty");
        for (member_position, &loop_id) in bundle.member_loop_ids.iter().enumerate() {
            sample_interleaved_position.insert(loop_id, (unit_position, member_position));
        }
    }

    let mut order = events.iter().collect::<Vec<_>>();
    order.sort_unstable_by_key(|event| match event.region {
        EventRegion::Control => (0, 0, 0, 0, 0, 0, event.event_id),
        EventRegion::Epoch(epoch_id) => {
            let phase = match event.kind {
                VectorEventKind::EpochEnter { .. } => 0,
                VectorEventKind::EpochExit { .. } => 4,
                _ => unreachable!("event-table checker restricts epoch events"),
            };
            (epoch_position[&epoch_id], phase, 0, 0, 0, 0, event.event_id)
        }
        EventRegion::LoopPre(loop_id)
        | EventRegion::Loop(loop_id)
        | EventRegion::LoopPost(loop_id) => {
            let epoch = epoch_position[&loop_epoch[&loop_id]];
            let (phase, sample) = match event.region {
                EventRegion::LoopPre(_) => (1, 0),
                EventRegion::Loop(_) => (
                    2,
                    usize::try_from(event.sample.expect("exec event has a sample"))
                        .expect("event table is bounded by usize"),
                ),
                EventRegion::LoopPost(_) => (3, 0),
                EventRegion::Control | EventRegion::Epoch(_) => unreachable!(),
            };
            if phase != 2 {
                let loop_position = if sample_major {
                    scalar_loop_position[&loop_id]
                } else {
                    vector_loop_position[&loop_id]
                };
                (epoch, phase, loop_position, 0, 0, 0, event.event_id)
            } else if sample_major {
                (
                    epoch,
                    phase,
                    sample,
                    scalar_loop_position[&loop_id],
                    0,
                    0,
                    event.event_id,
                )
            } else {
                let (unit_position, member_position) = sample_interleaved_position
                    .get(&loop_id)
                    .copied()
                    .unwrap_or((vector_loop_position[&loop_id], 0));
                (
                    epoch,
                    phase,
                    unit_position,
                    sample,
                    member_position,
                    0,
                    event.event_id,
                )
            }
        }
    });
    order.into_iter().map(|event| event.event_id).collect()
}

fn verify_required_dependencies(
    plan: &VectorPlan,
    state: Option<&VerifiedVectorStatePlan>,
    events: &[VectorEvent],
    dependencies: &[EventDependency],
    scalar_positions: &BTreeMap<u64, usize>,
) -> Result<(), VectorEventError> {
    let present = dependencies.iter().copied().collect::<BTreeSet<_>>();
    let contexts = context_events(events);
    let require = |before, after| {
        if before == after || present.contains(&EventDependency { before, after }) {
            Ok(())
        } else {
            Err(VectorEventError::DependencyMismatch)
        }
    };

    for local in contexts.values() {
        for pair in local.windows(2) {
            require(pair[0], pair[1])?;
        }
    }
    let epochs = sorted_epochs(plan);
    for (index, epoch) in epochs.iter().enumerate() {
        let boundaries = &contexts[&(EventRegion::Epoch(epoch.epoch_id), None)];
        let enter = boundaries[0];
        let exit = boundaries[1];
        if index == 0 {
            if let Some(control) = contexts.get(&(EventRegion::Control, None))
                && let Some(last) = control.last()
            {
                require(*last, enter)?;
            }
        } else {
            let previous = &contexts[&(EventRegion::Epoch(epochs[index - 1].epoch_id), None)];
            require(previous[1], enter)?;
        }
        for &loop_id in &epoch.loops {
            if let Some(pre) = contexts.get(&(EventRegion::LoopPre(loop_id), None))
                && let (Some(first), Some(last)) = (pre.first(), pre.last())
            {
                require(enter, *first)?;
                for sample in 0..plan.vec_size {
                    if let Some(local) = contexts.get(&(EventRegion::Loop(loop_id), Some(sample)))
                        && let Some(sample_first) = local.first()
                    {
                        require(*last, *sample_first)?;
                    }
                }
            }
            for sample in 0..plan.vec_size {
                if let Some(local) = contexts.get(&(EventRegion::Loop(loop_id), Some(sample)))
                    && let (Some(first), Some(last)) = (local.first(), local.last())
                {
                    require(enter, *first)?;
                    require(*last, exit)?;
                }
            }
            if let Some(post) = contexts.get(&(EventRegion::LoopPost(loop_id), None))
                && let (Some(first), Some(last)) = (post.first(), post.last())
            {
                for sample in 0..plan.vec_size {
                    if let Some(local) = contexts.get(&(EventRegion::Loop(loop_id), Some(sample)))
                        && let Some(sample_last) = local.last()
                    {
                        require(*sample_last, *first)?;
                    }
                }
                require(*last, exit)?;
            }
        }
    }
    for edge in plan.data_edges.iter().chain(&plan.effect_edges) {
        for sample in 0..plan.vec_size {
            let producer = contexts
                .get(&(EventRegion::Loop(edge.dependency), Some(sample)))
                .and_then(|local| local.last());
            let consumer = contexts
                .get(&(EventRegion::Loop(edge.consumer), Some(sample)))
                .and_then(|local| local.first());
            if let (Some(producer), Some(consumer)) = (producer, consumer) {
                require(*producer, *consumer)?;
            }
        }
    }

    let definitions = events
        .iter()
        .filter_map(|event| match event.kind {
            VectorEventKind::Definition { signal_id } => {
                Some(((event.region, signal_id, event.sample), event.event_id))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let loads = event_ids_by_transport(events, false);
    let transports = plan
        .transports
        .iter()
        .map(|transport| (transport.transport_id, transport))
        .collect::<BTreeMap<_, _>>();
    for event in events {
        match event.kind {
            VectorEventKind::TransportStore { transport_id } => {
                let transport = transports[&transport_id];
                require(
                    definitions[&(
                        EventRegion::Loop(transport.producer_loop),
                        transport.signal_id,
                        event.sample,
                    )],
                    event.event_id,
                )?;
                require(event.event_id, loads[&(transport_id, event.sample)])?;
            }
            VectorEventKind::Use {
                signal_id, source, ..
            } => {
                let source_event = match source {
                    EventUseSource::Control => {
                        definitions[&(EventRegion::Control, signal_id, None)]
                    }
                    EventUseSource::Loop(loop_id) => {
                        definitions[&(EventRegion::Loop(loop_id), signal_id, event.sample)]
                    }
                    EventUseSource::Transport(transport_id) => loads[&(transport_id, event.sample)],
                };
                require(source_event, event.event_id)?;
            }
            _ => {}
        }
    }

    let effects = events
        .iter()
        .filter_map(|event| match &event.kind {
            VectorEventKind::Effect { effect, .. } => Some((event.event_id, effect)),
            _ => None,
        })
        .collect::<Vec<_>>();
    for dependency in checker_required_effect_dependencies(&effects, scalar_positions) {
        require(dependency.before, dependency.after)?;
    }
    let recursion_steps = recursion_step_events(events);
    for ((loop_id, sample, group), event_id) in &recursion_steps {
        if *sample + 1 < plan.vec_size
            && let Some(next) = recursion_steps.get(&(*loop_id, *sample + 1, *group))
        {
            require(*event_id, *next)?;
        }
    }
    for dependency in managed_state_dependencies(plan, state, events, scalar_positions) {
        require(dependency.before, dependency.after)?;
    }
    Ok(())
}

fn derive_certificate(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: Option<&VerifiedVectorStatePlan>,
    limits: EventLimits,
) -> Result<EventOrderCertificate, VectorEventError> {
    verify_vector_plan(plan)?;
    if routed.plan() != plan {
        return Err(VectorEventError::RoutedPlanMismatch);
    }
    validate_layout(plan, routed)?;

    if state.is_some_and(|state| state.vector_plan() != plan) {
        return Err(VectorEventError::StatePlanMismatch);
    }
    let templates = event_templates(plan, routed, state)?;
    let checked_sample_count = producer_checked_sample_count(plan, &templates, state, limits)?;
    let mut checked_plan = plan.clone();
    checked_plan.vec_size = checked_sample_count;
    let needed = expanded_event_count(&checked_plan, &templates, state)?;
    let events = expand_events(&checked_plan, templates, state)?;
    debug_assert_eq!(events.len(), needed);
    if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
        eprintln!(
            "[vector-event-size] logical_samples={} checked_samples={} events={} complete_limit={} compact_limit={}",
            plan.vec_size,
            checked_sample_count,
            events.len(),
            limits.complete,
            limits.compact
        );
    }
    let contexts = context_events(&events);
    let scalar_loops = canonical_scalar_loops(&checked_plan);
    let vector_loops = routed_layout_loops(&checked_plan, routed);
    let scalar_order = build_order(&checked_plan, &contexts, &scalar_loops, true);
    let vector_order = build_order(&checked_plan, &contexts, &vector_loops, false);
    let dependencies = build_dependencies(&checked_plan, state, &events, &contexts, &scalar_order);

    Ok(EventOrderCertificate {
        sample_count: plan.vec_size,
        checked_sample_count,
        events,
        scalar_order,
        vector_order,
        dependencies,
    })
}

fn producer_checked_sample_count(
    plan: &VectorPlan,
    templates: &BTreeMap<EventRegion, Vec<VectorEventKind>>,
    state: Option<&VerifiedVectorStatePlan>,
    limits: EventLimits,
) -> Result<u64, VectorEventError> {
    let needed = expanded_event_count(plan, templates, state)?;
    if needed <= limits.complete {
        return Ok(plan.vec_size);
    }
    let basis = plan.vec_size.min(COMPACT_EVENT_SAMPLE_BASIS as u64);
    let mut compact_plan = plan.clone();
    compact_plan.vec_size = basis;
    let compact_needed = expanded_event_count(&compact_plan, templates, state)?;
    if compact_needed > limits.compact {
        return Err(VectorEventError::EventBoundExceeded {
            needed: compact_needed,
            limit: limits.compact,
        });
    }
    Ok(basis)
}

fn validate_layout(plan: &VectorPlan, routed: &VerifiedRoutedFir) -> Result<(), VectorEventError> {
    let loop_by_id = plan
        .loops
        .iter()
        .map(|record| (record.loop_id, record))
        .collect::<BTreeMap<_, _>>();
    let epoch_by_id = plan
        .epochs
        .iter()
        .map(|epoch| (epoch.epoch_id, epoch))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut positions = BTreeMap::new();
    let mut previous_rank = None;
    for (index, region) in routed.layout().loops().iter().enumerate() {
        let Some(record) = loop_by_id.get(&region.loop_id) else {
            return Err(VectorEventError::InvalidLayout {
                detail: "unknown loop",
                loop_id: region.loop_id,
            });
        };
        let Some(epoch) = epoch_by_id.get(&record.epoch_id) else {
            return Err(VectorEventError::InvalidLayout {
                detail: "unknown epoch",
                loop_id: region.loop_id,
            });
        };
        if !seen.insert(region.loop_id) {
            return Err(VectorEventError::InvalidLayout {
                detail: "duplicate loop",
                loop_id: region.loop_id,
            });
        }
        if region.epoch_id != record.epoch_id
            || region.epoch_rank != epoch.rank
            || region.kind != record.kind
            || region.roots != record.roots
        {
            return Err(VectorEventError::InvalidLayout {
                detail: "metadata mismatch",
                loop_id: region.loop_id,
            });
        }
        if previous_rank.is_some_and(|rank| rank > region.epoch_rank) {
            return Err(VectorEventError::InvalidLayout {
                detail: "epoch rank reversal",
                loop_id: region.loop_id,
            });
        }
        previous_rank = Some(region.epoch_rank);
        positions.insert(region.loop_id, index);
    }
    if seen.len() != plan.loops.len() {
        let missing = plan
            .loops
            .iter()
            .find(|record| !seen.contains(&record.loop_id))
            .map_or(u64::MAX, |record| record.loop_id);
        return Err(VectorEventError::InvalidLayout {
            detail: "missing loop",
            loop_id: missing,
        });
    }
    for edge in plan.data_edges.iter().chain(&plan.effect_edges) {
        if positions[&edge.dependency] >= positions[&edge.consumer] {
            return Err(VectorEventError::InvalidLayout {
                detail: "dependency reversal",
                loop_id: edge.consumer,
            });
        }
    }
    Ok(())
}

fn event_templates(
    plan: &VectorPlan,
    routed: &VerifiedRoutedFir,
    state: Option<&VerifiedVectorStatePlan>,
) -> Result<BTreeMap<EventRegion, Vec<VectorEventKind>>, VectorEventError> {
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let mut templates = BTreeMap::<EventRegion, Vec<VectorEventKind>>::new();
    let managed = state.map_or_else(BTreeSet::new, VerifiedVectorStatePlan::managed_resources);
    for definition in routed.trace().definitions() {
        let region = event_region(definition.region);
        templates
            .entry(region)
            .or_default()
            .push(VectorEventKind::Definition {
                signal_id: definition.signal_id,
            });
        for (index, effect) in signals[&definition.signal_id]
            .direct_effects
            .iter()
            .enumerate()
        {
            if effect_is_managed(effect, &managed) {
                continue;
            }
            let effect_index =
                u64::try_from(index).map_err(|_| VectorEventError::EventCountOverflow)?;
            templates
                .entry(region)
                .or_default()
                .push(VectorEventKind::Effect {
                    signal_id: definition.signal_id,
                    effect_index,
                    effect: effect.clone(),
                });
        }
    }
    let transports = plan
        .transports
        .iter()
        .map(|transport| (transport.transport_id, transport))
        .collect::<BTreeMap<_, _>>();
    for routed_transport in routed.trace().transports() {
        let transport = transports[&routed_transport.transport_id];
        templates
            .entry(EventRegion::Loop(transport.producer_loop))
            .or_default()
            .push(VectorEventKind::TransportStore {
                transport_id: transport.transport_id,
            });
        templates
            .entry(EventRegion::Loop(transport.consumer_loop))
            .or_default()
            .push(VectorEventKind::TransportLoad {
                transport_id: transport.transport_id,
            });
    }

    let mut uses = routed
        .trace()
        .uses()
        .iter()
        .map(|routed_use| {
            (
                routed_use.consumer_loop,
                routed_use.signal_id,
                event_use_source(routed_use.source),
            )
        })
        .collect::<Vec<_>>();
    uses.sort_unstable();
    let mut occurrences = BTreeMap::new();
    for (consumer_loop, signal_id, source) in uses {
        let occurrence = occurrences
            .entry((consumer_loop, signal_id, source))
            .or_insert(0_u64);
        templates
            .entry(EventRegion::Loop(consumer_loop))
            .or_default()
            .push(VectorEventKind::Use {
                signal_id,
                source,
                occurrence: *occurrence,
            });
        *occurrence = occurrence
            .checked_add(1)
            .ok_or(VectorEventError::EventCountOverflow)?;
    }
    for events in templates.values_mut() {
        events.sort();
    }
    Ok(templates)
}

fn expanded_event_count(
    plan: &VectorPlan,
    templates: &BTreeMap<EventRegion, Vec<VectorEventKind>>,
    state: Option<&VerifiedVectorStatePlan>,
) -> Result<usize, VectorEventError> {
    let mut total = plan
        .epochs
        .len()
        .checked_mul(2)
        .ok_or(VectorEventError::EventCountOverflow)?;
    total = total
        .checked_add(templates.get(&EventRegion::Control).map_or(0, Vec::len))
        .ok_or(VectorEventError::EventCountOverflow)?;
    let samples =
        usize::try_from(plan.vec_size).map_err(|_| VectorEventError::EventCountOverflow)?;
    for record in &plan.loops {
        let count = templates
            .get(&EventRegion::Loop(record.loop_id))
            .map_or(0, Vec::len);
        total = total
            .checked_add(
                count
                    .checked_mul(samples)
                    .ok_or(VectorEventError::EventCountOverflow)?,
            )
            .ok_or(VectorEventError::EventCountOverflow)?;
    }
    if let Some(state) = state {
        for phases in &state.plan().loops {
            total = total
                .checked_add(phases.pre.len())
                .and_then(|value| value.checked_add(phases.post.len()))
                .and_then(|value| {
                    phases
                        .exec
                        .len()
                        .checked_mul(samples)
                        .and_then(|exec| value.checked_add(exec))
                })
                .ok_or(VectorEventError::EventCountOverflow)?;
        }
    }
    Ok(total)
}

fn expand_events(
    plan: &VectorPlan,
    templates: BTreeMap<EventRegion, Vec<VectorEventKind>>,
    state: Option<&VerifiedVectorStatePlan>,
) -> Result<Vec<VectorEvent>, VectorEventError> {
    let mut keys = Vec::new();
    if let Some(control) = templates.get(&EventRegion::Control) {
        for kind in control {
            keys.push((EventRegion::Control, None, kind.clone()));
        }
    }
    for epoch in &plan.epochs {
        keys.push((
            EventRegion::Epoch(epoch.epoch_id),
            None,
            VectorEventKind::EpochEnter {
                epoch_id: epoch.epoch_id,
            },
        ));
        keys.push((
            EventRegion::Epoch(epoch.epoch_id),
            None,
            VectorEventKind::EpochExit {
                epoch_id: epoch.epoch_id,
            },
        ));
    }
    for record in &plan.loops {
        if let Some(loop_templates) = templates.get(&EventRegion::Loop(record.loop_id)) {
            for sample in 0..plan.vec_size {
                for kind in loop_templates {
                    keys.push((
                        EventRegion::Loop(record.loop_id),
                        Some(sample),
                        kind.clone(),
                    ));
                }
            }
        }
    }
    if let Some(state) = state {
        append_state_event_keys(&mut keys, state, plan.vec_size);
    }
    keys.sort();
    keys.into_iter()
        .enumerate()
        .map(|(event_id, (region, sample, kind))| {
            Ok(VectorEvent {
                event_id: u64::try_from(event_id)
                    .map_err(|_| VectorEventError::EventCountOverflow)?,
                region,
                sample,
                kind,
            })
        })
        .collect()
}

fn context_events(events: &[VectorEvent]) -> BTreeMap<(EventRegion, Option<u64>), Vec<u64>> {
    let mut contexts = BTreeMap::<_, Vec<_>>::new();
    for event in events {
        contexts
            .entry((event.region, event.sample))
            .or_default()
            .push(event.event_id);
    }
    contexts
}

fn canonical_scalar_loops(plan: &VectorPlan) -> BTreeMap<u64, Vec<u64>> {
    let mut result = BTreeMap::new();
    for epoch in sorted_epochs(plan) {
        let members = epoch.loops.iter().copied().collect::<BTreeSet<_>>();
        let mut remaining = members.clone();
        let mut order = Vec::with_capacity(members.len());
        while !remaining.is_empty() {
            let next = remaining
                .iter()
                .copied()
                .find(|candidate| {
                    plan.data_edges
                        .iter()
                        .chain(&plan.effect_edges)
                        .filter(|edge| {
                            edge.consumer == *candidate && members.contains(&edge.dependency)
                        })
                        .all(|edge| !remaining.contains(&edge.dependency))
                })
                .expect("verified epoch DAG has a ready loop");
            remaining.remove(&next);
            order.push(next);
        }
        result.insert(epoch.epoch_id, order);
    }
    result
}

fn routed_layout_loops(plan: &VectorPlan, routed: &VerifiedRoutedFir) -> BTreeMap<u64, Vec<u64>> {
    let mut result = plan
        .epochs
        .iter()
        .map(|epoch| (epoch.epoch_id, Vec::new()))
        .collect::<BTreeMap<_, _>>();
    for region in routed.layout().loops() {
        result
            .get_mut(&region.epoch_id)
            .expect("layout validation checked epoch")
            .push(region.loop_id);
    }
    result
}

fn build_order(
    plan: &VectorPlan,
    contexts: &BTreeMap<(EventRegion, Option<u64>), Vec<u64>>,
    loops_by_epoch: &BTreeMap<u64, Vec<u64>>,
    sample_major: bool,
) -> Vec<u64> {
    let mut order = contexts
        .get(&(EventRegion::Control, None))
        .cloned()
        .unwrap_or_default();
    for epoch in sorted_epochs(plan) {
        order.extend(
            contexts[&(EventRegion::Epoch(epoch.epoch_id), None)]
                .iter()
                .copied()
                .filter(|event_id| is_epoch_enter(*event_id, contexts, epoch.epoch_id)),
        );
        let loop_order = &loops_by_epoch[&epoch.epoch_id];
        append_epoch_events(plan, contexts, loop_order, &mut order, sample_major);
        order.extend(
            contexts[&(EventRegion::Epoch(epoch.epoch_id), None)]
                .iter()
                .copied()
                .filter(|event_id| !is_epoch_enter(*event_id, contexts, epoch.epoch_id)),
        );
    }
    order
}

fn append_epoch_events(
    plan: &VectorPlan,
    contexts: &BTreeMap<(EventRegion, Option<u64>), Vec<u64>>,
    loops: &[u64],
    order: &mut Vec<u64>,
    sample_major: bool,
) {
    for &loop_id in loops {
        append_context(contexts, EventRegion::LoopPre(loop_id), None, order);
    }
    if sample_major {
        for sample in 0..plan.vec_size {
            for &loop_id in loops {
                append_context(contexts, EventRegion::Loop(loop_id), Some(sample), order);
            }
        }
    } else {
        let group_by_member = plan
            .fused_serial_groups
            .iter()
            .flat_map(|group| {
                group
                    .member_loop_ids
                    .iter()
                    .map(move |&loop_id| (loop_id, group.group_id))
            })
            .collect::<BTreeMap<_, _>>();
        let bundle_by_member = plan
            .lockstep_bundles
            .iter()
            .flat_map(|bundle| {
                bundle
                    .member_loop_ids
                    .iter()
                    .map(move |&loop_id| (loop_id, bundle.bundle_id))
            })
            .collect::<BTreeMap<_, _>>();
        let mut emitted_groups = BTreeSet::new();
        let mut emitted_bundles = BTreeSet::new();
        for &loop_id in loops {
            if let Some(&group_id) = group_by_member.get(&loop_id) {
                if !emitted_groups.insert(group_id) {
                    continue;
                }
                let members = &plan
                    .fused_serial_groups
                    .iter()
                    .find(|group| group.group_id == group_id)
                    .expect("member map came from a verified fused group")
                    .member_loop_ids;
                for sample in 0..plan.vec_size {
                    for &member in loops {
                        if members.binary_search(&member).is_ok() {
                            append_context(
                                contexts,
                                EventRegion::Loop(member),
                                Some(sample),
                                order,
                            );
                        }
                    }
                }
            } else if let Some(&bundle_id) = bundle_by_member.get(&loop_id) {
                if !emitted_bundles.insert(bundle_id) {
                    continue;
                }
                let members = &plan
                    .lockstep_bundles
                    .iter()
                    .find(|bundle| bundle.bundle_id == bundle_id)
                    .expect("member map came from a verified lockstep bundle")
                    .member_loop_ids;
                for sample in 0..plan.vec_size {
                    for &member in loops {
                        if members.binary_search(&member).is_ok() {
                            append_context(
                                contexts,
                                EventRegion::Loop(member),
                                Some(sample),
                                order,
                            );
                        }
                    }
                }
            } else {
                for sample in 0..plan.vec_size {
                    append_context(contexts, EventRegion::Loop(loop_id), Some(sample), order);
                }
            }
        }
    }
    for &loop_id in loops {
        append_context(contexts, EventRegion::LoopPost(loop_id), None, order);
    }
}

fn append_context(
    contexts: &BTreeMap<(EventRegion, Option<u64>), Vec<u64>>,
    region: EventRegion,
    sample: Option<u64>,
    order: &mut Vec<u64>,
) {
    order.extend(
        contexts
            .get(&(region, sample))
            .into_iter()
            .flatten()
            .copied(),
    );
}

fn is_epoch_enter(
    event_id: u64,
    contexts: &BTreeMap<(EventRegion, Option<u64>), Vec<u64>>,
    epoch_id: u64,
) -> bool {
    contexts[&(EventRegion::Epoch(epoch_id), None)]
        .first()
        .is_some_and(|first| *first == event_id)
}

fn build_dependencies(
    plan: &VectorPlan,
    state: Option<&VerifiedVectorStatePlan>,
    events: &[VectorEvent],
    contexts: &BTreeMap<(EventRegion, Option<u64>), Vec<u64>>,
    scalar_order: &[u64],
) -> Vec<EventDependency> {
    let mut dependencies = BTreeSet::new();
    for event_ids in contexts.values() {
        for pair in event_ids.windows(2) {
            add_dependency(&mut dependencies, pair[0], pair[1]);
        }
    }

    let epochs = sorted_epochs(plan);
    for (index, epoch) in epochs.iter().enumerate() {
        let boundaries = &contexts[&(EventRegion::Epoch(epoch.epoch_id), None)];
        let enter = boundaries[0];
        let exit = boundaries[1];
        if index == 0 {
            if let Some(control) = contexts.get(&(EventRegion::Control, None))
                && let Some(last) = control.last()
            {
                add_dependency(&mut dependencies, *last, enter);
            }
        } else {
            let previous = &contexts[&(EventRegion::Epoch(epochs[index - 1].epoch_id), None)];
            add_dependency(&mut dependencies, previous[1], enter);
        }
        for &loop_id in &epoch.loops {
            if let Some(pre) = contexts.get(&(EventRegion::LoopPre(loop_id), None))
                && let (Some(first), Some(last)) = (pre.first(), pre.last())
            {
                add_dependency(&mut dependencies, enter, *first);
                for sample in 0..plan.vec_size {
                    if let Some(local) = contexts.get(&(EventRegion::Loop(loop_id), Some(sample)))
                        && let Some(sample_first) = local.first()
                    {
                        add_dependency(&mut dependencies, *last, *sample_first);
                    }
                }
            }
            for sample in 0..plan.vec_size {
                if let Some(local) = contexts.get(&(EventRegion::Loop(loop_id), Some(sample)))
                    && let (Some(first), Some(last)) = (local.first(), local.last())
                {
                    add_dependency(&mut dependencies, enter, *first);
                    add_dependency(&mut dependencies, *last, exit);
                }
            }
            if let Some(post) = contexts.get(&(EventRegion::LoopPost(loop_id), None))
                && let (Some(first), Some(last)) = (post.first(), post.last())
            {
                for sample in 0..plan.vec_size {
                    if let Some(local) = contexts.get(&(EventRegion::Loop(loop_id), Some(sample)))
                        && let Some(sample_last) = local.last()
                    {
                        add_dependency(&mut dependencies, *sample_last, *first);
                    }
                }
                add_dependency(&mut dependencies, *last, exit);
            }
        }
    }

    for edge in plan.data_edges.iter().chain(&plan.effect_edges) {
        add_loop_edge_dependencies(plan, contexts, *edge, &mut dependencies);
    }

    let by_id = events
        .iter()
        .map(|event| (event.event_id, event))
        .collect::<BTreeMap<_, _>>();
    let definitions = events
        .iter()
        .filter_map(|event| match event.kind {
            VectorEventKind::Definition { signal_id } => {
                Some(((event.region, signal_id, event.sample), event.event_id))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let stores = event_ids_by_transport(events, true);
    let loads = event_ids_by_transport(events, false);
    let transports = plan
        .transports
        .iter()
        .map(|transport| (transport.transport_id, transport))
        .collect::<BTreeMap<_, _>>();

    for event in events {
        match event.kind {
            VectorEventKind::TransportStore { transport_id } => {
                let transport = transports[&transport_id];
                let definition = definitions[&(
                    EventRegion::Loop(transport.producer_loop),
                    transport.signal_id,
                    event.sample,
                )];
                add_dependency(&mut dependencies, definition, event.event_id);
                add_dependency(
                    &mut dependencies,
                    event.event_id,
                    loads[&(transport_id, event.sample)],
                );
            }
            VectorEventKind::Use {
                signal_id, source, ..
            } => {
                let source_event = match source {
                    EventUseSource::Control => {
                        definitions[&(EventRegion::Control, signal_id, None)]
                    }
                    EventUseSource::Loop(loop_id) => {
                        definitions[&(EventRegion::Loop(loop_id), signal_id, event.sample)]
                    }
                    EventUseSource::Transport(transport_id) => loads[&(transport_id, event.sample)],
                };
                add_dependency(&mut dependencies, source_event, event.event_id);
            }
            _ => {}
        }
    }
    for ((transport_id, sample), store) in stores {
        add_dependency(&mut dependencies, store, loads[&(transport_id, sample)]);
    }

    let scalar_positions = positions(scalar_order);
    let effects = scalar_order
        .iter()
        .filter_map(|event_id| match &by_id[event_id].kind {
            VectorEventKind::Effect { effect, .. } => Some((*event_id, effect)),
            _ => None,
        })
        .collect::<Vec<_>>();
    dependencies.extend(producer_effect_dependencies(&effects));
    let recursion_steps = recursion_step_events(events);
    for ((loop_id, sample, group), event_id) in &recursion_steps {
        if *sample + 1 < plan.vec_size
            && let Some(next) = recursion_steps.get(&(*loop_id, *sample + 1, *group))
        {
            add_dependency(&mut dependencies, *event_id, *next);
        }
    }
    dependencies.extend(managed_state_dependencies(
        plan,
        state,
        events,
        &scalar_positions,
    ));
    dependencies.into_iter().collect()
}

/// Groups producer-side effect events by resource before materializing only
/// the conflicting pairs. Input order is the scalar event order.
fn producer_effect_dependencies(effects: &[(u64, &EffectAtom)]) -> BTreeSet<EventDependency> {
    let mut all = Vec::with_capacity(effects.len());
    let mut barriers = Vec::new();
    let mut states = BTreeMap::<StateResource, (Vec<u64>, Vec<u64>)>::new();
    let mut tables = BTreeMap::<u32, (Vec<u64>, Vec<u64>)>::new();
    let mut ui_writes = BTreeMap::<u32, Vec<u64>>::new();
    let mut output_writes = BTreeMap::<u32, Vec<u64>>::new();
    let positions = effects
        .iter()
        .enumerate()
        .map(|(position, (event_id, _))| (*event_id, position))
        .collect::<BTreeMap<_, _>>();

    for &(event_id, effect) in effects {
        all.push(event_id);
        match effect {
            EffectAtom::ReadState(resource) => {
                states.entry(resource.clone()).or_default().0.push(event_id);
            }
            EffectAtom::WriteState(resource) => {
                states.entry(resource.clone()).or_default().1.push(event_id);
            }
            EffectAtom::ReadTable(resource) => {
                tables.entry(*resource).or_default().0.push(event_id);
            }
            EffectAtom::WriteTable(resource) => {
                tables.entry(*resource).or_default().1.push(event_id);
            }
            EffectAtom::WriteUi(resource) => {
                ui_writes.entry(*resource).or_default().push(event_id);
            }
            EffectAtom::WriteOutput(resource) => {
                output_writes.entry(*resource).or_default().push(event_id);
            }
            EffectAtom::Foreign {
                purity: ForeignPurity::Impure | ForeignPurity::Unknown,
                ..
            } => barriers.push(event_id),
            EffectAtom::Foreign {
                purity: ForeignPurity::Pure,
                ..
            } => {}
        }
    }

    let mut dependencies = BTreeSet::new();
    let mut insert = |left: u64, right: u64| {
        if left == right {
            return;
        }
        let (before, after) = if positions[&left] < positions[&right] {
            (left, right)
        } else {
            (right, left)
        };
        dependencies.insert(EventDependency { before, after });
    };
    for (reads, writes) in states.values().chain(tables.values()) {
        for &read in reads {
            for &write in writes {
                insert(read, write);
            }
        }
        for (index, &left) in writes.iter().enumerate() {
            for &right in &writes[index + 1..] {
                insert(left, right);
            }
        }
    }
    for writes in ui_writes.values().chain(output_writes.values()) {
        for (index, &left) in writes.iter().enumerate() {
            for &right in &writes[index + 1..] {
                insert(left, right);
            }
        }
    }
    for &barrier in &barriers {
        for &other in &all {
            insert(barrier, other);
        }
    }
    dependencies
}

/// Independently reconstructs the required effect dependencies for the
/// checker. No producer grouping or producer result is consumed here.
fn checker_required_effect_dependencies(
    effects: &[(u64, &EffectAtom)],
    scalar_positions: &BTreeMap<u64, usize>,
) -> BTreeSet<EventDependency> {
    let mut every_effect = Vec::with_capacity(effects.len());
    let mut global_barriers = Vec::new();
    let mut state_accesses = BTreeMap::<StateResource, (Vec<u64>, Vec<u64>)>::new();
    let mut table_accesses = BTreeMap::<u32, (Vec<u64>, Vec<u64>)>::new();
    let mut ui_updates = BTreeMap::<u32, Vec<u64>>::new();
    let mut output_updates = BTreeMap::<u32, Vec<u64>>::new();

    for &(event_id, effect) in effects {
        every_effect.push(event_id);
        match effect {
            EffectAtom::ReadState(resource) => {
                state_accesses
                    .entry(resource.clone())
                    .or_default()
                    .0
                    .push(event_id);
            }
            EffectAtom::WriteState(resource) => {
                state_accesses
                    .entry(resource.clone())
                    .or_default()
                    .1
                    .push(event_id);
            }
            EffectAtom::ReadTable(resource) => {
                table_accesses
                    .entry(*resource)
                    .or_default()
                    .0
                    .push(event_id);
            }
            EffectAtom::WriteTable(resource) => {
                table_accesses
                    .entry(*resource)
                    .or_default()
                    .1
                    .push(event_id);
            }
            EffectAtom::WriteUi(resource) => {
                ui_updates.entry(*resource).or_default().push(event_id);
            }
            EffectAtom::WriteOutput(resource) => {
                output_updates.entry(*resource).or_default().push(event_id);
            }
            EffectAtom::Foreign {
                purity: ForeignPurity::Impure | ForeignPurity::Unknown,
                ..
            } => global_barriers.push(event_id),
            EffectAtom::Foreign {
                purity: ForeignPurity::Pure,
                ..
            } => {}
        }
    }

    let mut required = BTreeSet::new();
    let mut require_pair = |left: u64, right: u64| {
        if left == right {
            return;
        }
        let (before, after) = if scalar_positions[&left] < scalar_positions[&right] {
            (left, right)
        } else {
            (right, left)
        };
        required.insert(EventDependency { before, after });
    };
    for (reads, writes) in state_accesses.values().chain(table_accesses.values()) {
        for &reader in reads {
            for &writer in writes {
                require_pair(reader, writer);
            }
        }
        for (index, &left) in writes.iter().enumerate() {
            for &right in &writes[index + 1..] {
                require_pair(left, right);
            }
        }
    }
    for writes in ui_updates.values().chain(output_updates.values()) {
        for (index, &left) in writes.iter().enumerate() {
            for &right in &writes[index + 1..] {
                require_pair(left, right);
            }
        }
    }
    for &barrier in &global_barriers {
        for &other in &every_effect {
            require_pair(barrier, other);
        }
    }
    required
}

fn add_loop_edge_dependencies(
    plan: &VectorPlan,
    contexts: &BTreeMap<(EventRegion, Option<u64>), Vec<u64>>,
    edge: LoopEdge,
    dependencies: &mut BTreeSet<EventDependency>,
) {
    for sample in 0..plan.vec_size {
        let producer = contexts
            .get(&(EventRegion::Loop(edge.dependency), Some(sample)))
            .and_then(|events| events.last());
        let consumer = contexts
            .get(&(EventRegion::Loop(edge.consumer), Some(sample)))
            .and_then(|events| events.first());
        if let (Some(producer), Some(consumer)) = (producer, consumer) {
            add_dependency(dependencies, *producer, *consumer);
        }
    }
}

fn event_ids_by_transport(
    events: &[VectorEvent],
    stores: bool,
) -> BTreeMap<(u64, Option<u64>), u64> {
    events
        .iter()
        .filter_map(|event| {
            let transport_id = match event.kind {
                VectorEventKind::TransportStore { transport_id } if stores => transport_id,
                VectorEventKind::TransportLoad { transport_id } if !stores => transport_id,
                _ => return None,
            };
            Some(((transport_id, event.sample), event.event_id))
        })
        .collect()
}

fn managed_state_dependencies(
    plan: &VectorPlan,
    state: Option<&VerifiedVectorStatePlan>,
    events: &[VectorEvent],
    scalar_positions: &BTreeMap<u64, usize>,
) -> BTreeSet<EventDependency> {
    let Some(state) = state else {
        return BTreeSet::new();
    };
    let signals = plan
        .signals
        .iter()
        .map(|signal| (signal.signal_id, signal))
        .collect::<BTreeMap<_, _>>();
    let mut reads = BTreeMap::<(StateResource, u64), Vec<u64>>::new();
    let mut writes = BTreeMap::<(StateResource, u64), Vec<u64>>::new();
    for event in events {
        let Some(sample) = event.sample else {
            continue;
        };
        match &event.kind {
            VectorEventKind::Definition { signal_id } => {
                for effect in &signals[signal_id].effects {
                    if let EffectAtom::ReadState(resource) = effect {
                        reads
                            .entry((resource.clone(), sample))
                            .or_default()
                            .push(event.event_id);
                    }
                }
            }
            VectorEventKind::StateTransition { action } => {
                for resource in state_action_resources(state, action) {
                    writes
                        .entry((resource, sample))
                        .or_default()
                        .push(event.event_id);
                }
            }
            _ => {}
        }
    }

    let mut dependencies = BTreeSet::new();
    for ((resource, sample), writer_ids) in &writes {
        for &writer_id in writer_ids {
            if let Some(reader_ids) = reads.get(&(resource.clone(), *sample)) {
                for &reader_id in reader_ids {
                    if scalar_positions[&reader_id] < scalar_positions[&writer_id] {
                        add_dependency(&mut dependencies, reader_id, writer_id);
                    }
                }
            }
            let Some(next_sample) = sample.checked_add(1).filter(|next| *next < plan.vec_size)
            else {
                continue;
            };
            if let Some(reader_ids) = reads.get(&(resource.clone(), next_sample)) {
                for &reader_id in reader_ids {
                    if scalar_positions[&writer_id] < scalar_positions[&reader_id] {
                        add_dependency(&mut dependencies, writer_id, reader_id);
                    }
                }
            }
            if let Some(next_writer_ids) = writes.get(&(resource.clone(), next_sample)) {
                for &next_writer_id in next_writer_ids {
                    if scalar_positions[&writer_id] < scalar_positions[&next_writer_id] {
                        add_dependency(&mut dependencies, writer_id, next_writer_id);
                    }
                }
            }
        }
    }
    dependencies
}

fn state_action_resources(
    state: &VerifiedVectorStatePlan,
    action: &VectorStateAction,
) -> Vec<StateResource> {
    match action {
        VectorStateAction::DelayWrite { signal_id } => vec![StateResource::Signal {
            owner: u32::try_from(*signal_id).expect("verified signal id fits u32"),
            cell: super::vector_analysis::StateCell::Delay,
        }],
        VectorStateAction::RecursionStep { group } => state
            .plan()
            .recursions
            .iter()
            .find(|recursion| recursion.group == *group)
            .into_iter()
            .flat_map(|recursion| {
                recursion
                    .projections
                    .iter()
                    .map(move |projection| StateResource::Recursion {
                        group: u32::try_from(*group).expect("verified recursion group fits u32"),
                        projection: u32::try_from(projection.index)
                            .expect("verified recursion projection fits u32"),
                    })
            })
            .collect(),
        VectorStateAction::PrefixWrite { signal_id } => vec![StateResource::Signal {
            owner: u32::try_from(*signal_id).expect("verified signal id fits u32"),
            cell: super::vector_analysis::StateCell::Prefix,
        }],
        VectorStateAction::WaveformAdvance { signal_id } => vec![StateResource::Signal {
            owner: u32::try_from(*signal_id).expect("verified signal id fits u32"),
            cell: super::vector_analysis::StateCell::WaveformIndex,
        }],
        VectorStateAction::DelayRegisterLoad { .. }
        | VectorStateAction::DelayRegisterStore { .. }
        | VectorStateAction::DelayCopyIn { .. }
        | VectorStateAction::DelayCopyOut { .. }
        | VectorStateAction::DelayRingAdvance { .. }
        | VectorStateAction::DelayRingSaveAdvance { .. } => Vec::new(),
    }
}

fn recursion_step_events(events: &[VectorEvent]) -> BTreeMap<(u64, u64, u64), u64> {
    events
        .iter()
        .filter_map(|event| {
            let EventRegion::Loop(loop_id) = event.region else {
                return None;
            };
            let VectorEventKind::StateTransition {
                action: VectorStateAction::RecursionStep { group },
            } = &event.kind
            else {
                return None;
            };
            Some(((loop_id, event.sample?, *group), event.event_id))
        })
        .collect()
}

fn add_dependency(dependencies: &mut BTreeSet<EventDependency>, before: u64, after: u64) {
    if before != after {
        dependencies.insert(EventDependency { before, after });
    }
}

fn validate_order(
    which: &'static str,
    events: &[VectorEvent],
    order: &[u64],
) -> Result<(), VectorEventError> {
    let expected = events
        .iter()
        .map(|event| event.event_id)
        .collect::<BTreeSet<_>>();
    let actual = order.iter().copied().collect::<BTreeSet<_>>();
    if order.len() != events.len() || actual != expected {
        Err(VectorEventError::InvalidOrder { which })
    } else {
        Ok(())
    }
}

fn positions(order: &[u64]) -> BTreeMap<u64, usize> {
    order
        .iter()
        .enumerate()
        .map(|(position, event_id)| (*event_id, position))
        .collect()
}

fn sorted_epochs(plan: &VectorPlan) -> Vec<&super::vector_verify::EpochRecord> {
    let mut epochs = plan.epochs.iter().collect::<Vec<_>>();
    epochs.sort_unstable_by_key(|epoch| (epoch.rank, epoch.epoch_id));
    epochs
}

fn event_region(region: VectorRegion) -> EventRegion {
    match region {
        VectorRegion::Control => EventRegion::Control,
        VectorRegion::Loop(loop_id) => EventRegion::Loop(loop_id),
    }
}

fn event_use_source(source: RoutedUseSource) -> EventUseSource {
    match source {
        RoutedUseSource::Direct(VectorRegion::Control) => EventUseSource::Control,
        RoutedUseSource::Direct(VectorRegion::Loop(loop_id)) => EventUseSource::Loop(loop_id),
        RoutedUseSource::Transport(transport_id) => EventUseSource::Transport(transport_id),
    }
}

#[cfg(test)]
mod tests {
    use fir::{FirBuilder, FirStore, FirType};
    use propagate::ClockDomainTable;
    use tlib::TreeArena;

    use super::*;
    use crate::clk_env::annotate;
    use crate::schedule::SchedulingStrategy;
    use crate::signal_fir::decoration_verify::{
        VerifiedDecorationCertificate, certify_decorations,
    };
    use crate::signal_fir::pv_slice::build_pv_signals;
    use crate::signal_fir::vector_analysis::{
        ForeignResource, ForeignTypeCode, StateCell, StateResource,
    };
    use crate::signal_fir::vector_plan::{build_vector_plan, verified_vector_plan_for_test};
    use crate::signal_fir::vector_route::{RouteResolution, VectorRouteSession};
    use crate::signal_fir::vector_state::{
        LoopStatePhases, RecursionProjectionTransition, RecursionTransition,
        VECTOR_STATE_PLAN_VERSION, VectorStateAction, VectorStatePlan, build_vector_state_plan,
        verified_vector_state_plan_for_test,
    };
    use crate::signal_fir::vector_verify::{
        EpochRecord, IsoLeafMapping, IsoRootWitness, LockstepBundleRecord, LockstepLaneRecord,
        LoopKind, LoopRecord, Placement, Rate, SignalRecord, TransportRecord, ValueType,
        VecSafeWitness, Vectorability, WitnessKind,
    };

    const ALL_STRATEGIES: [SchedulingStrategy; 4] = [
        SchedulingStrategy::DepthFirst,
        SchedulingStrategy::BreadthFirst,
        SchedulingStrategy::Special,
        SchedulingStrategy::ReverseBreadthFirst,
    ];

    fn certify(arena: &TreeArena, roots: &[signals::SigId]) -> VerifiedDecorationCertificate {
        let prepared = crate::signal_prepare::prepare_signals_for_fir_verified(
            arena,
            roots,
            &ui::UiProgram::empty(),
        )
        .unwrap();
        let clocks = annotate(
            prepared.arena(),
            &ClockDomainTable::new(),
            prepared.outputs(),
        )
        .unwrap();
        certify_decorations(&prepared, &clocks).unwrap()
    }

    #[test]
    fn grouped_effect_dependencies_exhaustively_match_literal_pairing() {
        fn literal_dependencies(
            effects: &[(u64, &EffectAtom)],
            scalar_positions: &BTreeMap<u64, usize>,
        ) -> BTreeSet<EventDependency> {
            let mut result = BTreeSet::new();
            for (index, &(left_id, left)) in effects.iter().enumerate() {
                for &(right_id, right) in &effects[index + 1..] {
                    if effects_conflict(left, right) {
                        let (before, after) =
                            if scalar_positions[&left_id] < scalar_positions[&right_id] {
                                (left_id, right_id)
                            } else {
                                (right_id, left_id)
                            };
                        result.insert(EventDependency { before, after });
                    }
                }
            }
            result
        }

        let state_a = StateResource::Signal {
            owner: 1,
            cell: StateCell::Delay,
        };
        let state_b = StateResource::Signal {
            owner: 2,
            cell: StateCell::Delay,
        };
        let foreign = |name: &str, purity| EffectAtom::Foreign {
            resource: ForeignResource::Variable {
                name: name.to_owned(),
                value_type: ForeignTypeCode(1),
            },
            purity,
        };
        let atoms = vec![
            EffectAtom::ReadState(state_a.clone()),
            EffectAtom::WriteState(state_a),
            EffectAtom::ReadState(state_b.clone()),
            EffectAtom::WriteState(state_b),
            EffectAtom::ReadTable(3),
            EffectAtom::WriteTable(3),
            EffectAtom::ReadTable(4),
            EffectAtom::WriteTable(4),
            EffectAtom::WriteUi(5),
            EffectAtom::WriteUi(5),
            EffectAtom::WriteUi(6),
            EffectAtom::WriteOutput(7),
            EffectAtom::WriteOutput(7),
            foreign("impure", ForeignPurity::Impure),
            foreign("unknown", ForeignPurity::Unknown),
            foreign("pure", ForeignPurity::Pure),
        ];

        for mask in 0_u64..(1_u64 << atoms.len()) {
            let selected = atoms
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1_u64 << index) != 0)
                .map(|(index, effect)| (u64::try_from(index).unwrap(), effect))
                .collect::<Vec<_>>();
            for reverse in [false, true] {
                let mut scalar_effects = selected.clone();
                if reverse {
                    scalar_effects.reverse();
                }
                let scalar_positions = scalar_effects
                    .iter()
                    .enumerate()
                    .map(|(position, (event_id, _))| (*event_id, position))
                    .collect::<BTreeMap<_, _>>();
                let expected = literal_dependencies(&scalar_effects, &scalar_positions);
                assert_eq!(
                    producer_effect_dependencies(&scalar_effects),
                    expected,
                    "producer mismatch for mask {mask:#x}, reverse={reverse}"
                );

                let mut checker_input = scalar_effects.clone();
                checker_input.sort_unstable_by_key(|(event_id, _)| *event_id);
                assert_eq!(
                    checker_required_effect_dependencies(&checker_input, &scalar_positions),
                    expected,
                    "checker mismatch for mask {mask:#x}, reverse={reverse}"
                );
            }
        }
    }

    fn pure_transport_plan() -> VerifiedVectorPlan {
        verified_vector_plan_for_test(VectorPlan {
            schema_version: crate::signal_fir::vector_verify::VECTOR_PLAN_SCHEMA_VERSION,
            lockstep_bundles: Vec::new(),
            vec_size: 3,
            signals: vec![
                signal(0, Placement::Owned(0), vec![]),
                signal(1, Placement::Owned(1), vec![]),
            ],
            loops: vec![vector_loop(0, vec![0]), vector_loop(1, vec![1])],
            epochs: vec![EpochRecord {
                epoch_id: 0,
                rank: 0,
                loops: vec![0, 1],
            }],
            transports: vec![TransportRecord {
                transport_id: 0,
                stable_name: "transport_s0_l0_l1".to_owned(),
                signal_id: 0,
                producer_loop: 0,
                consumer_loop: 1,
                element_type: ValueType::Real,
                length: 3,
                layout: crate::signal_fir::vector_verify::TransportLayout::Planar,
            }],
            data_edges: vec![LoopEdge {
                consumer: 1,
                dependency: 0,
            }],
            effect_edges: vec![],
            vec_safe_witnesses: vec![
                VecSafeWitness {
                    loop_id: 0,
                    witness_kind: WitnessKind::Pointwise,
                },
                VecSafeWitness {
                    loop_id: 1,
                    witness_kind: WitnessKind::Pointwise,
                },
            ],
            fused_serial_groups: vec![],
        })
    }

    fn compact_lockstep_plan() -> VerifiedVectorPlan {
        verified_vector_plan_for_test(VectorPlan {
            schema_version: crate::signal_fir::vector_verify::VECTOR_PLAN_SCHEMA_VERSION,
            vec_size: 32,
            signals: (0..2)
                .map(|lane| signal(10 + lane, Placement::Owned(lane), vec![]))
                .collect(),
            loops: (0..2)
                .map(|lane| LoopRecord {
                    loop_id: lane,
                    stable_name: format!("lockstep_lane_{lane}"),
                    kind: LoopKind::Lockstep { width: 2 },
                    roots: vec![10 + lane],
                    epoch_id: 0,
                })
                .collect(),
            epochs: vec![EpochRecord {
                epoch_id: 0,
                rank: 0,
                loops: vec![0, 1],
            }],
            transports: vec![],
            data_edges: vec![],
            effect_edges: vec![],
            vec_safe_witnesses: (0..2)
                .map(|loop_id| VecSafeWitness {
                    loop_id,
                    witness_kind: WitnessKind::SerialStateInternal,
                })
                .collect(),
            fused_serial_groups: vec![],
            lockstep_bundles: vec![LockstepBundleRecord {
                bundle_id: 0,
                representative_loop_id: 0,
                member_loop_ids: vec![0, 1],
                lanes: (0..2)
                    .map(|lane| LockstepLaneRecord {
                        loop_id: lane,
                        recursion_group: 20 + lane,
                        roots: vec![IsoRootWitness {
                            representative_root: 10,
                            lane_root: 10 + lane,
                            shape_hash: 0x55,
                            leaf_mapping: vec![IsoLeafMapping {
                                representative_signal_id: 10,
                                lane_signal_id: 10 + lane,
                            }],
                        }],
                    })
                    .collect(),
            }],
        })
    }

    fn split_state_plan() -> VerifiedVectorPlan {
        let resource = StateResource::Signal {
            owner: 7,
            cell: StateCell::Delay,
        };
        split_effect_plan(
            EffectAtom::WriteState(resource.clone()),
            EffectAtom::ReadState(resource),
        )
    }

    fn split_effect_plan(left: EffectAtom, right: EffectAtom) -> VerifiedVectorPlan {
        split_plan_with_signals(vec![
            signal(0, Placement::Owned(0), vec![left]),
            signal(1, Placement::Owned(1), vec![right]),
        ])
    }

    fn split_plan_with_signals(signals: Vec<SignalRecord>) -> VerifiedVectorPlan {
        verified_vector_plan_for_test(VectorPlan {
            schema_version: crate::signal_fir::vector_verify::VECTOR_PLAN_SCHEMA_VERSION,
            lockstep_bundles: Vec::new(),
            vec_size: 3,
            signals,
            loops: vec![serial_loop(0, vec![0]), serial_loop(1, vec![1])],
            epochs: vec![EpochRecord {
                epoch_id: 0,
                rank: 0,
                loops: vec![0, 1],
            }],
            transports: vec![],
            data_edges: vec![],
            effect_edges: vec![LoopEdge {
                consumer: 1,
                dependency: 0,
            }],
            vec_safe_witnesses: vec![
                VecSafeWitness {
                    loop_id: 0,
                    witness_kind: WitnessKind::SerialStateInternal,
                },
                VecSafeWitness {
                    loop_id: 1,
                    witness_kind: WitnessKind::SerialStateInternal,
                },
            ],
            fused_serial_groups: vec![],
        })
    }

    fn colocated_state_plan() -> VerifiedVectorPlan {
        let resource = StateResource::Signal {
            owner: 7,
            cell: StateCell::Delay,
        };
        verified_vector_plan_for_test(VectorPlan {
            schema_version: crate::signal_fir::vector_verify::VECTOR_PLAN_SCHEMA_VERSION,
            lockstep_bundles: Vec::new(),
            vec_size: 3,
            signals: vec![
                signal(
                    0,
                    Placement::Owned(0),
                    vec![EffectAtom::WriteState(resource.clone())],
                ),
                signal(
                    1,
                    Placement::Owned(0),
                    vec![EffectAtom::ReadState(resource)],
                ),
            ],
            loops: vec![serial_loop(0, vec![0, 1])],
            epochs: vec![EpochRecord {
                epoch_id: 0,
                rank: 0,
                loops: vec![0],
            }],
            transports: vec![],
            data_edges: vec![],
            effect_edges: vec![],
            vec_safe_witnesses: vec![VecSafeWitness {
                loop_id: 0,
                witness_kind: WitnessKind::SerialStateInternal,
            }],
            fused_serial_groups: vec![],
        })
    }

    fn recursive_event_plan() -> (VerifiedVectorPlan, VerifiedVectorStatePlan) {
        let projection0 = StateResource::Recursion {
            group: 7,
            projection: 0,
        };
        let projection1 = StateResource::Recursion {
            group: 7,
            projection: 1,
        };
        let plan = verified_vector_plan_for_test(VectorPlan {
            schema_version: crate::signal_fir::vector_verify::VECTOR_PLAN_SCHEMA_VERSION,
            lockstep_bundles: Vec::new(),
            vec_size: 3,
            signals: vec![
                signal(
                    0,
                    Placement::Owned(0),
                    vec![
                        EffectAtom::ReadState(projection0.clone()),
                        EffectAtom::WriteState(projection0),
                    ],
                ),
                signal(
                    1,
                    Placement::Owned(0),
                    vec![
                        EffectAtom::ReadState(projection1.clone()),
                        EffectAtom::WriteState(projection1),
                    ],
                ),
            ],
            loops: vec![LoopRecord {
                loop_id: 0,
                stable_name: "loop_rec_7".to_owned(),
                kind: LoopKind::Recursive(7),
                roots: vec![0, 1],
                epoch_id: 0,
            }],
            epochs: vec![EpochRecord {
                epoch_id: 0,
                rank: 0,
                loops: vec![0],
            }],
            transports: vec![],
            data_edges: vec![],
            effect_edges: vec![],
            vec_safe_witnesses: vec![VecSafeWitness {
                loop_id: 0,
                witness_kind: WitnessKind::SerialStateInternal,
            }],
            fused_serial_groups: vec![],
        });
        let state = verified_vector_state_plan_for_test(
            VectorStatePlan {
                schema_version: VECTOR_STATE_PLAN_VERSION,
                vec_size: 3,
                max_copy_delay: 16,
                loops: vec![LoopStatePhases {
                    loop_id: 0,
                    pre: vec![],
                    exec: vec![VectorStateAction::RecursionStep { group: 7 }],
                    post: vec![],
                }],
                delays: vec![],
                recursions: vec![RecursionTransition {
                    group: 7,
                    loop_id: 0,
                    projections: vec![
                        RecursionProjectionTransition {
                            index: 0,
                            signal_ids: vec![0],
                            value_signal_id: 0,
                        },
                        RecursionProjectionTransition {
                            index: 1,
                            signal_ids: vec![1],
                            value_signal_id: 1,
                        },
                    ],
                }],
                lockstep_register_bundles: vec![],
                prefixes: vec![],
                waveforms: vec![],
                no_op_resources: vec![],
            },
            &plan,
        );
        (plan, state)
    }

    fn empty_state_plan(plan: &VerifiedVectorPlan) -> VerifiedVectorStatePlan {
        verified_vector_state_plan_for_test(
            VectorStatePlan {
                schema_version: VECTOR_STATE_PLAN_VERSION,
                vec_size: plan.plan().vec_size,
                max_copy_delay: 16,
                loops: vec![],
                delays: vec![],
                recursions: vec![],
                lockstep_register_bundles: vec![],
                prefixes: vec![],
                waveforms: vec![],
                no_op_resources: vec![],
            },
            plan,
        )
    }

    fn compact_lockstep_state_plan(plan: &VerifiedVectorPlan) -> VerifiedVectorStatePlan {
        verified_vector_state_plan_for_test(
            VectorStatePlan {
                schema_version: VECTOR_STATE_PLAN_VERSION,
                vec_size: plan.plan().vec_size,
                max_copy_delay: 16,
                loops: (0..2)
                    .map(|lane| LoopStatePhases {
                        loop_id: lane,
                        pre: vec![],
                        exec: vec![VectorStateAction::RecursionStep { group: 20 + lane }],
                        post: vec![],
                    })
                    .collect(),
                delays: vec![],
                recursions: (0..2)
                    .map(|lane| RecursionTransition {
                        group: 20 + lane,
                        loop_id: lane,
                        projections: vec![RecursionProjectionTransition {
                            index: 0,
                            signal_ids: vec![10 + lane],
                            value_signal_id: 10 + lane,
                        }],
                    })
                    .collect(),
                lockstep_register_bundles: vec![],
                prefixes: vec![],
                waveforms: vec![],
                no_op_resources: vec![],
            },
            plan,
        )
    }

    fn signal(signal_id: u64, placement: Placement, effects: Vec<EffectAtom>) -> SignalRecord {
        SignalRecord {
            signal_id,
            value_type: ValueType::Real,
            structural: false,
            rate: Rate::Samp,
            vectorability: if effects.is_empty() {
                Vectorability::Vect
            } else {
                Vectorability::Scal
            },
            clock_id: 0,
            duplicable: effects.is_empty(),
            direct_effects: effects.clone(),
            effects,
            placement,
        }
    }

    fn vector_loop(loop_id: u64, roots: Vec<u64>) -> LoopRecord {
        LoopRecord {
            loop_id,
            stable_name: format!("loop_{loop_id}"),
            kind: LoopKind::Vectorizable,
            roots,
            epoch_id: 0,
        }
    }

    #[test]
    fn event_bound_precheck_accepts_a_small_plan_without_certifying_it() {
        let plan = pure_transport_plan();
        let state = empty_state_plan(&plan);
        assert_eq!(
            precheck_state_event_bound(&plan, &state, EventLimits::uniform(14)),
            Ok(())
        );
    }

    #[test]
    fn event_bound_precheck_rejects_when_even_the_compact_basis_is_too_large() {
        let plan = pure_transport_plan();
        let state = empty_state_plan(&plan);
        assert_eq!(
            precheck_state_event_bound(&plan, &state, EventLimits::uniform(9)),
            Err(VectorEventError::EventLowerBoundExceeded {
                minimum: 10,
                limit: 9,
            })
        );
    }

    fn serial_loop(loop_id: u64, roots: Vec<u64>) -> LoopRecord {
        LoopRecord {
            loop_id,
            stable_name: format!("serial_{loop_id}"),
            kind: LoopKind::Island(loop_id),
            roots,
            epoch_id: 0,
        }
    }

    fn route(
        plan: &VerifiedVectorPlan,
        strategy: SchedulingStrategy,
        with_transport_use: bool,
    ) -> VerifiedRoutedFir {
        let mut store = FirStore::new();
        let (mut session, _) =
            VectorRouteSession::new(plan, strategy, FirType::Float32, &mut store).unwrap();
        let loop_order = session
            .layout()
            .loops()
            .iter()
            .map(|region| region.loop_id)
            .collect::<Vec<_>>();
        for loop_id in loop_order {
            let roots = session
                .plan()
                .loops
                .iter()
                .find(|record| record.loop_id == loop_id)
                .unwrap()
                .roots
                .clone();
            if with_transport_use && loop_id == 1 {
                assert!(matches!(
                    session.resolve_in_loop(1, 0, &mut store).unwrap(),
                    RouteResolution::Value(_)
                ));
            }
            for signal_id in roots {
                let value = FirBuilder::new(&mut store).float32(signal_id as f32);
                session
                    .define_in_loop(loop_id, signal_id, value, &mut store)
                    .unwrap();
            }
        }
        session.finish(&store).unwrap()
    }

    fn route_all_transports(
        plan: &VerifiedVectorPlan,
        strategy: SchedulingStrategy,
    ) -> VerifiedRoutedFir {
        let mut store = FirStore::new();
        let (mut session, _) =
            VectorRouteSession::new(plan, strategy, FirType::Float32, &mut store).unwrap();
        let signals = session.plan().signals.clone();
        for signal in signals
            .iter()
            .filter(|signal| signal.placement == Placement::Control)
        {
            let value = routed_test_value(signal, &mut store);
            session
                .define_control(signal.signal_id, value, &store)
                .unwrap();
        }
        let loop_order = session
            .layout()
            .loops()
            .iter()
            .map(|region| region.loop_id)
            .collect::<Vec<_>>();
        let mut inline_defined = BTreeSet::new();
        for loop_id in loop_order {
            let incoming = session
                .plan()
                .transports
                .iter()
                .filter(|transport| transport.consumer_loop == loop_id)
                .map(|transport| transport.signal_id)
                .collect::<Vec<_>>();
            for signal_id in incoming {
                session
                    .resolve_in_loop(loop_id, signal_id, &mut store)
                    .unwrap();
            }
            for signal in &signals {
                let should_define = match signal.placement {
                    Placement::Owned(owner) => owner == loop_id,
                    Placement::Inline => inline_defined.insert(signal.signal_id),
                    Placement::Control => false,
                };
                if !should_define {
                    continue;
                }
                let value = routed_test_value(signal, &mut store);
                session
                    .define_in_loop(loop_id, signal.signal_id, value, &mut store)
                    .unwrap();
            }
        }
        session.finish(&store).unwrap()
    }

    fn routed_test_value(signal: &SignalRecord, store: &mut FirStore) -> fir::FirId {
        let mut builder = FirBuilder::new(store);
        match signal.value_type {
            ValueType::Int => builder.int32(0),
            ValueType::Real => builder.float32(0.0),
            ValueType::Sound | ValueType::Tuple(_) => {
                panic!("event route fixture does not synthesize handle or tuple FIR")
            }
        }
    }

    #[test]
    fn pure_transport_is_fission_safe_for_all_scheduling_strategies() {
        let plan = pure_transport_plan();
        for strategy in ALL_STRATEGIES {
            let routed = route(&plan, strategy, true);
            let verified =
                build_event_order_certificate(&plan, &routed, DEFAULT_EVENT_LIMITS).unwrap();
            let certificate = verified.certificate();
            assert_eq!(certificate.sample_count(), 3);
            assert_eq!(certificate.events().len(), 17);
            assert_ne!(certificate.scalar_order(), certificate.vector_order());
            assert!(certificate.events().iter().any(|event| matches!(
                event.kind,
                VectorEventKind::TransportStore { transport_id: 0 }
            )));
            assert!(certificate.events().iter().any(|event| matches!(
                event.kind,
                VectorEventKind::TransportLoad { transport_id: 0 }
            )));
        }
    }

    #[test]
    fn independent_checker_rejects_order_and_dependency_mutations() {
        let plan = pure_transport_plan();
        let routed = route(&plan, SchedulingStrategy::DepthFirst, true);
        let verified = build_event_order_certificate(&plan, &routed, DEFAULT_EVENT_LIMITS).unwrap();

        let mut order_mutation = verified.certificate().clone();
        order_mutation.vector_order.swap(2, 3);
        assert_eq!(
            verify_event_order_certificate(
                plan.plan(),
                &routed,
                &order_mutation,
                DEFAULT_EVENT_LIMITS
            ),
            Err(VectorEventError::VectorOrderMismatch)
        );

        let mut dependency_mutation = verified.into_certificate();
        dependency_mutation.dependencies.pop();
        assert_eq!(
            verify_event_order_certificate(
                plan.plan(),
                &routed,
                &dependency_mutation,
                DEFAULT_EVENT_LIMITS
            ),
            Err(VectorEventError::DependencyMismatch)
        );
    }

    #[test]
    fn general_routed_plan_uses_two_sample_basis_only_when_complete_chunk_exceeds_bound() {
        let plan = pure_transport_plan();
        let routed = route(&plan, SchedulingStrategy::DepthFirst, true);
        let complete = build_event_order_certificate(&plan, &routed, EventLimits::uniform(17))
            .expect("complete routed certificate");
        assert_eq!(complete.certificate().checked_sample_count(), 3);
        assert!(!complete.certificate().is_compact());

        let compact = build_event_order_certificate(&plan, &routed, EventLimits::uniform(16))
            .expect("compact routed certificate");
        assert_eq!(compact.certificate().sample_count(), 3);
        assert_eq!(compact.certificate().checked_sample_count(), 2);
        assert!(compact.certificate().is_compact());

        let split_budget = build_event_order_certificate(&plan, &routed, EventLimits::new(11, 12))
            .expect("separate compact budget");
        assert!(split_budget.certificate().is_compact());

        assert_eq!(
            build_event_order_certificate(&plan, &routed, EventLimits::uniform(11)),
            Err(VectorEventError::EventBoundExceeded {
                needed: 12,
                limit: 11,
            })
        );
    }

    #[test]
    fn expanded_and_compact_general_routed_models_have_the_same_two_sample_projection() {
        fn retained(event: &VectorEvent) -> bool {
            event.sample.is_none_or(|sample| sample < 2)
        }

        fn event_key(event: &VectorEvent) -> EventKey {
            (event.region, event.sample, event.kind.clone())
        }

        fn projected_order(certificate: &EventOrderCertificate, order: &[u64]) -> Vec<EventKey> {
            let by_id = certificate
                .events
                .iter()
                .map(|event| (event.event_id, event))
                .collect::<BTreeMap<_, _>>();
            order
                .iter()
                .map(|event_id| by_id[event_id])
                .filter(|event| retained(event))
                .map(event_key)
                .collect()
        }

        fn projected_dependencies(
            certificate: &EventOrderCertificate,
        ) -> BTreeSet<(EventKey, EventKey)> {
            let by_id = certificate
                .events
                .iter()
                .map(|event| (event.event_id, event))
                .collect::<BTreeMap<_, _>>();
            certificate
                .dependencies
                .iter()
                .filter_map(|dependency| {
                    let before = by_id[&dependency.before];
                    let after = by_id[&dependency.after];
                    (retained(before) && retained(after))
                        .then(|| (event_key(before), event_key(after)))
                })
                .collect()
        }

        let plan = pure_transport_plan();
        let routed = route(&plan, SchedulingStrategy::DepthFirst, true);
        let expanded = build_event_order_certificate(&plan, &routed, EventLimits::uniform(17))
            .expect("expanded certificate")
            .into_certificate();
        let compact = build_event_order_certificate(&plan, &routed, EventLimits::uniform(16))
            .expect("compact certificate")
            .into_certificate();

        assert_eq!(
            expanded
                .events
                .iter()
                .filter(|event| retained(event))
                .map(event_key)
                .collect::<Vec<_>>(),
            compact.events.iter().map(event_key).collect::<Vec<_>>()
        );
        assert_eq!(
            projected_order(&expanded, &expanded.scalar_order),
            projected_order(&compact, &compact.scalar_order)
        );
        assert_eq!(
            projected_order(&expanded, &expanded.vector_order),
            projected_order(&compact, &compact.vector_order)
        );
        assert_eq!(
            projected_dependencies(&expanded),
            projected_dependencies(&compact)
        );
    }

    #[test]
    fn lockstep_uses_two_sample_compact_basis_when_full_chunk_exceeds_bound() {
        let plan = compact_lockstep_plan();
        let routed = route(&plan, SchedulingStrategy::DepthFirst, false);
        let verified = build_event_order_certificate(&plan, &routed, EventLimits::uniform(10))
            .expect("compact lockstep certificate");
        let certificate = verified.certificate();
        assert_eq!(certificate.sample_count(), 32);
        assert_eq!(certificate.checked_sample_count(), 2);
        assert!(certificate.is_compact());
        assert_eq!(certificate.events().len(), 6);
    }

    #[test]
    fn compact_checker_rejects_sample_basis_and_template_mutations() {
        let plan = compact_lockstep_plan();
        let routed = route(&plan, SchedulingStrategy::DepthFirst, false);
        let verified = build_event_order_certificate(&plan, &routed, EventLimits::uniform(10))
            .expect("compact lockstep certificate");

        let mut basis_mutation = verified.certificate().clone();
        basis_mutation.checked_sample_count = 1;
        assert_eq!(
            verify_event_order_certificate(
                plan.plan(),
                &routed,
                &basis_mutation,
                EventLimits::uniform(10)
            ),
            Err(VectorEventError::CompactRepetitionMismatch)
        );

        let mut template_mutation = verified.into_certificate();
        let sample_one = template_mutation
            .events
            .iter_mut()
            .find(|event| event.sample == Some(1))
            .expect("sample-one template");
        sample_one.kind = VectorEventKind::Definition { signal_id: 999 };
        assert_eq!(
            verify_event_order_certificate(
                plan.plan(),
                &routed,
                &template_mutation,
                EventLimits::uniform(10)
            ),
            Err(VectorEventError::EventTableMismatch)
        );
    }

    #[test]
    fn compact_checker_rejects_a_missing_carried_recursion_edge() {
        let plan = compact_lockstep_plan();
        let state = compact_lockstep_state_plan(&plan);
        let routed = route(&plan, SchedulingStrategy::DepthFirst, false);
        let verified =
            build_state_event_order_certificate(&plan, &routed, &state, EventLimits::uniform(12))
                .expect("compact state certificate");
        assert!(verified.certificate().is_compact());

        let mut mutation = verified.into_certificate();
        let recursion_steps = recursion_step_events(&mutation.events);
        let before = recursion_steps[&(0, 0, 20)];
        let after = recursion_steps[&(0, 1, 20)];
        mutation
            .dependencies
            .retain(|edge| *edge != EventDependency { before, after });
        assert_eq!(
            verify_state_event_order_certificate(
                plan.plan(),
                &routed,
                &state,
                &mutation,
                EventLimits::uniform(12)
            ),
            Err(VectorEventError::DependencyMismatch)
        );
    }

    #[test]
    fn general_recursive_plan_compacts_and_keeps_the_carried_state_boundary() {
        let (plan, state) = recursive_event_plan();
        let routed = route(&plan, SchedulingStrategy::DepthFirst, false);
        let verified =
            build_state_event_order_certificate(&plan, &routed, &state, EventLimits::new(0, 64))
                .expect("compact general recursion certificate");
        assert!(verified.certificate().is_compact());

        let mut mutation = verified.into_certificate();
        let recursion_steps = recursion_step_events(&mutation.events);
        let before = recursion_steps[&(0, 0, 7)];
        let after = recursion_steps[&(0, 1, 7)];
        mutation
            .dependencies
            .retain(|edge| *edge != EventDependency { before, after });
        assert_eq!(
            verify_state_event_order_certificate(
                plan.plan(),
                &routed,
                &state,
                &mutation,
                EventLimits::new(0, 64)
            ),
            Err(VectorEventError::DependencyMismatch)
        );
    }

    #[test]
    fn cross_loop_carried_state_is_rejected_despite_an_effect_edge() {
        let plan = split_state_plan();
        let routed = route(&plan, SchedulingStrategy::DepthFirst, false);
        assert!(matches!(
            build_event_order_certificate(&plan, &routed, DEFAULT_EVENT_LIMITS),
            Err(VectorEventError::FissionSafeViolation { .. })
        ));
    }

    #[test]
    fn conflicting_observable_effects_in_separate_loops_are_rejected() {
        let plan = split_effect_plan(EffectAtom::WriteOutput(0), EffectAtom::WriteOutput(0));
        let routed = route(&plan, SchedulingStrategy::DepthFirst, false);
        assert!(matches!(
            build_event_order_certificate(&plan, &routed, DEFAULT_EVENT_LIMITS),
            Err(VectorEventError::FissionSafeViolation { .. })
        ));
    }

    #[test]
    fn a_transitive_carrier_of_an_effect_manufactures_no_conflict() {
        // Same two-loop shape as the rejected case above, but the second
        // signal only contains the performer in its subtree: its transitive
        // set holds the atom while its direct set is empty. Attributing the
        // operation to the carrier as well is what made `mixer` report a
        // write-after-write on a UI zone the compiler stores once.
        let atom = EffectAtom::WriteOutput(0);
        let carrier = SignalRecord {
            signal_id: 1,
            value_type: ValueType::Real,
            structural: false,
            rate: Rate::Samp,
            vectorability: Vectorability::Scal,
            clock_id: 0,
            duplicable: false,
            direct_effects: Vec::new(),
            effects: vec![atom.clone()],
            placement: Placement::Owned(1),
        };
        let plan = split_plan_with_signals(vec![
            signal(0, Placement::Owned(0), vec![atom.clone()]),
            carrier,
        ]);
        let routed = route(&plan, SchedulingStrategy::DepthFirst, false);
        let verified = build_event_order_certificate(&plan, &routed, DEFAULT_EVENT_LIMITS)
            .expect("a carrier must not conflict with the performer it inherits from");
        let performers = verified
            .certificate()
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                VectorEventKind::Effect {
                    signal_id, effect, ..
                } if *effect == atom => Some(*signal_id),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            performers,
            BTreeSet::from([0]),
            "only the performing signal may emit the effect event"
        );
    }

    #[test]
    fn conflicting_state_colocated_in_one_serial_loop_is_accepted() {
        let plan = colocated_state_plan();
        let routed = route(&plan, SchedulingStrategy::DepthFirst, false);
        build_event_order_certificate(&plan, &routed, DEFAULT_EVENT_LIMITS).unwrap();
    }

    #[test]
    fn p6_delay_phases_refine_managed_effects_for_all_strategies() {
        let (arena, y, z) = build_pv_signals(20);
        let decorations = certify(&arena, &[y, z]);
        let plan = build_vector_plan(&decorations, 3).unwrap();
        let state = build_vector_state_plan(&decorations, &plan, 16).unwrap();
        let delayed_signal = state
            .plan()
            .delays
            .iter()
            .find(|delay| delay.max_delay == 20)
            .unwrap()
            .signal_id;

        for strategy in ALL_STRATEGIES {
            let routed = route_all_transports(&plan, strategy);
            let verified =
                build_state_event_order_certificate(&plan, &routed, &state, DEFAULT_EVENT_LIMITS)
                    .unwrap();
            let certificate = verified.certificate();
            assert!(certificate.events().iter().any(|event| matches!(
                event.kind,
                VectorEventKind::StateTransition {
                    action: VectorStateAction::DelayRingAdvance { signal_id }
                } if signal_id == delayed_signal
            )));
            assert!(certificate.events().iter().any(|event| matches!(
                event.kind,
                VectorEventKind::StateTransition {
                    action: VectorStateAction::DelayRingSaveAdvance { signal_id }
                } if signal_id == delayed_signal
            )));
            assert!(!certificate.events().iter().any(|event| matches!(
                &event.kind,
                VectorEventKind::Effect {
                    effect: EffectAtom::ReadState(StateResource::Signal {
                        owner,
                        cell: StateCell::Delay,
                    }) | EffectAtom::WriteState(StateResource::Signal {
                        owner,
                        cell: StateCell::Delay,
                    }),
                    ..
                } if u64::from(*owner) == delayed_signal
            )));
        }
    }

    #[test]
    fn p6_recursion_steps_are_sample_ordered_for_all_strategies() {
        let (plan, state) = recursive_event_plan();
        let group = state.plan().recursions[0].group;

        for strategy in ALL_STRATEGIES {
            let routed = route_all_transports(&plan, strategy);
            let verified =
                build_state_event_order_certificate(&plan, &routed, &state, DEFAULT_EVENT_LIMITS)
                    .unwrap();
            let certificate = verified.certificate();
            let mut steps = certificate
                .events()
                .iter()
                .filter(|event| {
                    matches!(
                        event.kind,
                        VectorEventKind::StateTransition {
                            action: VectorStateAction::RecursionStep { group: event_group }
                        } if event_group == group
                    )
                })
                .map(|event| (event.sample.unwrap(), event.event_id))
                .collect::<Vec<_>>();
            steps.sort_unstable();
            assert_eq!(steps.len(), 3);
            for pair in steps.windows(2) {
                assert!(certificate.dependencies().contains(&EventDependency {
                    before: pair[0].1,
                    after: pair[1].1,
                }));
            }
        }
    }
}
