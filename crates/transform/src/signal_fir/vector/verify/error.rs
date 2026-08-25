//! `VectorPlanError`: the stable error taxonomy of the plan checker.

use super::model::*;
use std::fmt;

/// Why [`verify_vector_plan`](super::check::verify_vector_plan) rejected a
/// plan. One variant per checked
/// obligation, so each has a demonstrated rejecting mutation (plan provenance: §8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VectorPlanError {
    /// The verifier accepts only the exact v2 schema.
    UnsupportedSchema {
        /// The schema version the plan actually declared.
        found: u32,
    },
    /// `vec_size` must be positive.
    VecSizeZero,
    /// A set-like array is not in its required canonical order (also catches
    /// duplicates, since canonical order is *strictly* ascending).
    NotCanonical {
        /// Which set-like array broke canonical order.
        what: &'static str,
        /// Index of the first element that is not strictly greater than its
        /// predecessor.
        at: usize,
    },
    /// A loop appears in more than one epoch, or a plan loop is in none.
    EpochCoverageMismatch {
        /// The loop that is not owned by exactly one epoch.
        loop_id: u64,
    },
    /// An epoch lists a loop id that is not a plan loop.
    EpochLoopUnknown {
        /// The epoch listing the unknown loop.
        epoch_id: u64,
        /// The listed loop id absent from the plan.
        loop_id: u64,
    },
    /// A signal placed `Owned(l)` is absent from `l`'s roots.
    OwnedSignalNotRoot {
        /// The owned signal missing from the loop's roots.
        signal_id: u64,
        /// The loop the signal's placement names as owner.
        loop_id: u64,
    },
    /// A root of loop `l` is not placed `Owned(l)`.
    RootWithoutOwnership {
        /// The root signal whose placement is not `Owned` of its loop.
        signal_id: u64,
        /// The loop that lists the signal as a root.
        loop_id: u64,
    },
    /// A root references a signal id that is not a plan signal.
    RootUnknownSignal {
        /// The loop whose root list holds the dangling id.
        loop_id: u64,
        /// The referenced signal id absent from the plan.
        signal_id: u64,
    },
    /// An `Inline`-placed signal is not `duplicable`.
    InlineNotDuplicable {
        /// The inline-placed signal that is not duplicable.
        signal_id: u64,
    },
    /// The producer-supplied `duplicable` bit disagrees with the effect facts.
    DuplicabilityMismatch {
        /// The signal whose `duplicable` bit contradicts its effects.
        signal_id: u64,
    },
    /// A loop's redundant `epoch_id` disagrees with canonical epoch membership.
    LoopEpochMismatch {
        /// The loop with the inconsistent epoch declaration.
        loop_id: u64,
        /// The epoch id the loop record declares.
        declared: u64,
        /// The epoch that actually lists the loop as a member.
        actual: u64,
    },
    /// A data/effect edge references a loop id that is not a plan loop.
    EdgeEndpointUnknown {
        /// The edge with the dangling endpoint.
        edge: LoopEdge,
        /// The endpoint loop id absent from the plan.
        missing: u64,
    },
    /// A loop depends on itself (an instantaneous self-edge).
    LoopSelfEdge {
        /// The loop that is both consumer and dependency of one edge.
        loop_id: u64,
    },
    /// The induced graph of one epoch contains a cycle.
    EpochNotAcyclic {
        /// The epoch whose induced dependency graph is cyclic.
        epoch_id: u64,
        /// The loops left unscheduled after the topological sort stalled.
        remaining: Vec<u64>,
    },
    /// Two loops have conflicting effects but neither is ordered before the
    /// other by the combined data/effect relation.
    UnorderedEffectConflict {
        /// One of the two mutually unordered conflicting loops.
        left: u64,
        /// The other mutually unordered conflicting loop.
        right: u64,
    },
    /// A transport's producer and consumer loops are the same.
    TransportSelfLoop {
        /// The transport whose producer equals its consumer.
        transport_id: u64,
    },
    /// A transport's element type does not equal its signal's value type.
    TransportTypeMismatch {
        /// The transport with the mismatched element type.
        transport_id: u64,
    },
    /// A transport's array length does not equal `vec_size`.
    TransportLengthMismatch {
        /// The transport with the mismatched length.
        transport_id: u64,
    },
    /// An interleaved transport has no matching lockstep width.
    TransportLayoutMismatch {
        /// The transport whose interleaved layout has no matching bundle.
        transport_id: u64,
    },
    /// A lockstep bundle has fewer than two lanes or inconsistent width.
    LockstepWidthMismatch {
        /// The bundle whose declared width disagrees with its lanes.
        bundle_id: u64,
    },
    /// A lockstep bundle references a missing loop or repeats a member.
    LockstepMemberMismatch {
        /// The bundle with the invalid member list.
        bundle_id: u64,
        /// The offending member loop id.
        loop_id: u64,
    },
    /// A lane record does not correspond exactly to one bundle member.
    LockstepLaneMismatch {
        /// The bundle whose lane list disagrees with its members.
        bundle_id: u64,
        /// The lane loop without a matching member (or vice versa).
        loop_id: u64,
    },
    /// Two candidate lanes are connected in the epoch dependence graph.
    LockstepDependentLanes {
        /// The bundle holding the dependent lanes.
        bundle_id: u64,
        /// One of the two dependency-connected lanes.
        left: u64,
        /// The other dependency-connected lane.
        right: u64,
    },
    /// Two candidate lanes do not share the same epoch and clock.
    LockstepDomainMismatch {
        /// The bundle holding the mismatched lanes.
        bundle_id: u64,
        /// One of the two lanes disagreeing on epoch or clock.
        left: u64,
        /// The other lane of the disagreeing pair.
        right: u64,
    },
    /// Two candidate lanes have non-commuting effects.
    LockstepEffectConflict {
        /// The bundle holding the conflicting lanes.
        bundle_id: u64,
        /// One of the two lanes with conflicting effects.
        left: u64,
        /// The other lane of the conflicting pair.
        right: u64,
    },
    /// A root/leaf witness is not canonical or references signals outside its
    /// declared lane roots. Prepared-tree shape is checked by the second gate.
    LockstepIsoWitnessMismatch {
        /// The bundle whose lane carries the invalid witness.
        bundle_id: u64,
        /// The lane loop with the invalid isomorphism witness.
        loop_id: u64,
    },
    /// A cross-epoch edge whose dependency epoch has a strictly greater rank
    /// than its consumer epoch (a barrier run backwards).
    BarrierViolation {
        /// The cross-epoch edge that runs against barrier order.
        edge: LoopEdge,
    },
    /// A `Recursive`/`Island` loop carries a `pointwise` witness, or is
    /// otherwise asserted vector-safe in a way that contradicts its serial
    /// kind.
    SerialLoopNotSerial {
        /// The serial loop asserted vector-safe.
        loop_id: u64,
    },
    /// A `Vectorizable` loop has no `VecSafe` witness.
    VectorizableWithoutWitness {
        /// The vectorizable loop missing a witness.
        loop_id: u64,
    },
    /// A vectorizable loop's roots do not satisfy the concrete `VecSafe` rule.
    VectorizableNotSafe {
        /// The vectorizable loop that fails the `VecSafe` check.
        loop_id: u64,
    },
    /// A `VecSafe` witness references a loop id that is not a plan loop.
    WitnessUnknownLoop {
        /// The referenced loop id absent from the plan.
        loop_id: u64,
    },
    /// A transport references a signal or loop id that is not in the plan.
    TransportUnknownRef {
        /// The transport holding the dangling reference.
        transport_id: u64,
        /// The referenced signal or loop id absent from the plan.
        missing: u64,
    },
    /// A required set-like fused-group field is empty.
    FusedGroupEmpty {
        /// The group with the empty required field.
        group_id: u64,
        /// Which required field is empty.
        what: &'static str,
    },
    /// A fused group references a loop absent from the plan.
    FusedGroupUnknownLoop {
        /// The group holding the dangling loop reference.
        group_id: u64,
        /// The referenced loop id absent from the plan.
        loop_id: u64,
    },
    /// The owner is not included in the group's members.
    FusedGroupOwnerNotMember {
        /// The group whose owner is outside its member list.
        group_id: u64,
        /// The declared owner loop missing from the members.
        owner_loop_id: u64,
    },
    /// One loop belongs to two fused serial groups.
    FusedGroupLoopOverlap {
        /// The loop claimed by more than one group.
        loop_id: u64,
    },
    /// A fused group references a signal absent from the plan.
    FusedGroupUnknownSignal {
        /// The group holding the dangling signal reference.
        group_id: u64,
        /// The referenced signal id absent from the plan.
        signal_id: u64,
    },
    /// A grouped signal is not owned by one of the group's member loops.
    FusedGroupSignalOutside {
        /// The group listing the outside signal.
        group_id: u64,
        /// The signal not owned by any member loop.
        signal_id: u64,
    },
    /// A rematerialized transport id is absent from the plan.
    FusedGroupUnknownTransport {
        /// The group holding the dangling transport reference.
        group_id: u64,
        /// The referenced transport id absent from the plan.
        transport_id: u64,
    },
    /// A rematerialized transport does not stay within its group.
    FusedGroupTransportOutside {
        /// The group whose internal transport crosses its boundary.
        group_id: u64,
        /// The transport with an endpoint outside the member loops.
        transport_id: u64,
    },
    /// A rematerialized transport does not carry one of the certified delayed
    /// reads.
    FusedGroupTransportNotDelayedRead {
        /// The group listing the non-delayed-read transport.
        group_id: u64,
        /// The transport whose signal is not a certified delayed read.
        transport_id: u64,
    },
    /// The selected owner is not the canonical owner of a certified carrier.
    FusedGroupOwnerNotStateCarrier {
        /// The group with the non-canonical owner.
        group_id: u64,
        /// The declared owner loop that owns no certified carrier.
        owner_loop_id: u64,
    },
    /// The decoration facts do not identify the declared carrier as delayed
    /// state.
    FusedGroupCarrierNotDelayedState {
        /// The group declaring the uncertified carrier.
        group_id: u64,
        /// The declared carrier signal not certified as delayed state.
        signal_id: u64,
    },
    /// A delayed read lacks the declared `DepKind::Delayed` carrier edge.
    FusedGroupDelayedDependencyMissing {
        /// The group declaring the delayed read.
        group_id: u64,
        /// The delayed-read signal missing its delayed carrier edge.
        signal_id: u64,
    },
    /// A same-sample path from a delayed read to its recursive writer leaves
    /// the fused serial group.
    FusedGroupPathIncomplete {
        /// The group whose member set misses a data-path loop.
        group_id: u64,
        /// The on-path loop absent from the group's members.
        loop_id: u64,
    },
    /// A declared state writer does not match its recursive member loop.
    FusedGroupStateWriterMismatch {
        /// The group declaring the mismatched writer.
        group_id: u64,
        /// The writer signal that matches no recursive member loop.
        signal_id: u64,
    },
    /// A recursive member loop has no matching projection writer in the group.
    FusedGroupRecursiveMemberMissingWriter {
        /// The group whose writer set is incomplete.
        group_id: u64,
        /// The recursive member loop without a state writer.
        loop_id: u64,
    },
    /// Grouped signals or loops cross incompatible clock domains.
    FusedGroupClockMismatch {
        /// The group spanning more than one clock domain.
        group_id: u64,
    },
    /// An active chunk transport still materializes a delayed read internally.
    FusedGroupDangerousTransportPresent {
        /// The group in which the delayed read is still materialized.
        group_id: u64,
        /// The chunk transport that should have been rematerialized.
        transport_id: u64,
    },
    /// An independently reconstructed immediate-delay crossing is uncovered.
    FusedGroupDangerousCrossingMissing {
        /// The loop producing the delayed state.
        producer: u64,
        /// The loop consuming it in the same sample.
        consumer: u64,
    },
}
impl fmt::Display for VectorPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema { found } => {
                write!(f, "unsupported vector-plan schema {found}")
            }
            Self::VecSizeZero => write!(f, "vec_size must be positive"),
            Self::NotCanonical { what, at } => {
                write!(f, "{what} is not strictly ascending at index {at}")
            }
            Self::EpochCoverageMismatch { loop_id } => {
                write!(f, "loop {loop_id} is not owned by exactly one epoch")
            }
            Self::EpochLoopUnknown { epoch_id, loop_id } => {
                write!(f, "epoch {epoch_id} lists unknown loop {loop_id}")
            }
            Self::OwnedSignalNotRoot { signal_id, loop_id } => write!(
                f,
                "signal {signal_id} is Owned({loop_id}) but not a root of that loop"
            ),
            Self::RootWithoutOwnership { signal_id, loop_id } => write!(
                f,
                "signal {signal_id} is a root of loop {loop_id} but not placed Owned({loop_id})"
            ),
            Self::RootUnknownSignal { loop_id, signal_id } => {
                write!(f, "loop {loop_id} root {signal_id} is not a plan signal")
            }
            Self::InlineNotDuplicable { signal_id } => {
                write!(f, "Inline signal {signal_id} is not duplicable")
            }
            Self::DuplicabilityMismatch { signal_id } => write!(
                f,
                "signal {signal_id} duplicable bit disagrees with its effects"
            ),
            Self::LoopEpochMismatch {
                loop_id,
                declared,
                actual,
            } => write!(
                f,
                "loop {loop_id} declares epoch {declared} but belongs to epoch {actual}"
            ),
            Self::EdgeEndpointUnknown { edge, missing } => {
                write!(f, "edge {edge:?} references unknown loop {missing}")
            }
            Self::LoopSelfEdge { loop_id } => write!(f, "loop {loop_id} depends on itself"),
            Self::EpochNotAcyclic {
                epoch_id,
                remaining,
            } => write!(
                f,
                "epoch {epoch_id} induced graph has a cycle: {remaining:?}"
            ),
            Self::UnorderedEffectConflict { left, right } => write!(
                f,
                "loops {left} and {right} have conflicting unordered effects"
            ),
            Self::TransportSelfLoop { transport_id } => {
                write!(f, "transport {transport_id} producer == consumer")
            }
            Self::TransportTypeMismatch { transport_id } => write!(
                f,
                "transport {transport_id} element type != its signal's value type"
            ),
            Self::TransportLengthMismatch { transport_id } => {
                write!(f, "transport {transport_id} length != vec_size")
            }
            Self::TransportLayoutMismatch { transport_id } => {
                write!(f, "transport {transport_id} has an invalid lockstep layout")
            }
            Self::LockstepWidthMismatch { bundle_id } => {
                write!(f, "lockstep bundle {bundle_id} has an inconsistent width")
            }
            Self::LockstepMemberMismatch { bundle_id, loop_id } => write!(
                f,
                "lockstep bundle {bundle_id} has invalid member loop {loop_id}"
            ),
            Self::LockstepLaneMismatch { bundle_id, loop_id } => write!(
                f,
                "lockstep bundle {bundle_id} has invalid lane loop {loop_id}"
            ),
            Self::LockstepDependentLanes {
                bundle_id,
                left,
                right,
            } => write!(
                f,
                "lockstep bundle {bundle_id} lanes {left} and {right} are dependency-connected"
            ),
            Self::LockstepDomainMismatch {
                bundle_id,
                left,
                right,
            } => write!(
                f,
                "lockstep bundle {bundle_id} lanes {left} and {right} disagree on epoch or clock"
            ),
            Self::LockstepEffectConflict {
                bundle_id,
                left,
                right,
            } => write!(
                f,
                "lockstep bundle {bundle_id} lanes {left} and {right} have conflicting effects"
            ),
            Self::LockstepIsoWitnessMismatch { bundle_id, loop_id } => write!(
                f,
                "lockstep bundle {bundle_id} lane {loop_id} has an invalid isomorphism witness"
            ),
            Self::BarrierViolation { edge } => {
                write!(f, "cross-epoch edge {edge:?} runs a barrier backwards")
            }
            Self::SerialLoopNotSerial { loop_id } => {
                write!(
                    f,
                    "serial (recursive/island) loop {loop_id} asserted vector-safe"
                )
            }
            Self::VectorizableWithoutWitness { loop_id } => {
                write!(f, "vectorizable loop {loop_id} has no VecSafe witness")
            }
            Self::VectorizableNotSafe { loop_id } => {
                write!(f, "vectorizable loop {loop_id} does not satisfy VecSafe")
            }
            Self::WitnessUnknownLoop { loop_id } => {
                write!(f, "VecSafe witness references unknown loop {loop_id}")
            }
            Self::TransportUnknownRef {
                transport_id,
                missing,
            } => write!(
                f,
                "transport {transport_id} references unknown id {missing}"
            ),
            Self::FusedGroupEmpty { group_id, what } => {
                write!(f, "fused group {group_id} has empty {what}")
            }
            Self::FusedGroupUnknownLoop { group_id, loop_id } => {
                write!(
                    f,
                    "fused group {group_id} references unknown loop {loop_id}"
                )
            }
            Self::FusedGroupOwnerNotMember {
                group_id,
                owner_loop_id,
            } => write!(
                f,
                "fused group {group_id} owner loop {owner_loop_id} is not a member"
            ),
            Self::FusedGroupLoopOverlap { loop_id } => {
                write!(f, "loop {loop_id} belongs to more than one fused group")
            }
            Self::FusedGroupUnknownSignal {
                group_id,
                signal_id,
            } => write!(
                f,
                "fused group {group_id} references unknown signal {signal_id}"
            ),
            Self::FusedGroupSignalOutside {
                group_id,
                signal_id,
            } => write!(
                f,
                "fused group {group_id} signal {signal_id} is outside its member loops"
            ),
            Self::FusedGroupUnknownTransport {
                group_id,
                transport_id,
            } => write!(
                f,
                "fused group {group_id} references unknown transport {transport_id}"
            ),
            Self::FusedGroupTransportOutside {
                group_id,
                transport_id,
            } => write!(
                f,
                "fused group {group_id} transport {transport_id} crosses its member boundary"
            ),
            Self::FusedGroupTransportNotDelayedRead {
                group_id,
                transport_id,
            } => write!(
                f,
                "fused group {group_id} transport {transport_id} is not a delayed read"
            ),
            Self::FusedGroupOwnerNotStateCarrier {
                group_id,
                owner_loop_id,
            } => write!(
                f,
                "fused group {group_id} owner loop {owner_loop_id} is not the canonical state-carrier owner"
            ),
            Self::FusedGroupCarrierNotDelayedState {
                group_id,
                signal_id,
            } => write!(
                f,
                "fused group {group_id} carrier {signal_id} is not certified delayed state"
            ),
            Self::FusedGroupDelayedDependencyMissing {
                group_id,
                signal_id,
            } => write!(
                f,
                "fused group {group_id} read {signal_id} lacks its delayed carrier dependency"
            ),
            Self::FusedGroupPathIncomplete { group_id, loop_id } => {
                write!(f, "fused group {group_id} omits data-path loop {loop_id}")
            }
            Self::FusedGroupStateWriterMismatch {
                group_id,
                signal_id,
            } => write!(
                f,
                "fused group {group_id} writer {signal_id} does not match a recursive member loop"
            ),
            Self::FusedGroupRecursiveMemberMissingWriter { group_id, loop_id } => write!(
                f,
                "fused group {group_id} recursive member loop {loop_id} has no state writer"
            ),
            Self::FusedGroupClockMismatch { group_id } => {
                write!(f, "fused group {group_id} crosses clock domains")
            }
            Self::FusedGroupDangerousTransportPresent {
                group_id,
                transport_id,
            } => write!(
                f,
                "fused group {group_id} still materializes delayed read transport {transport_id}"
            ),
            Self::FusedGroupDangerousCrossingMissing { producer, consumer } => write!(
                f,
                "state-mediated immediate delay crossing {producer} -> {consumer} is not covered by a fused group"
            ),
        }
    }
}
impl std::error::Error for VectorPlanError {}
