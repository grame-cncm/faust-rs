//! State-plan vocabulary (storage, transitions, phases), the verified
//! wrapper, and the error taxonomy (schema v4).

use super::check::managed_resources;
#[cfg(test)]
use super::check::verify_phases;
use crate::signal_fir::vector::analysis::StateResource;
#[cfg(test)]
use crate::signal_fir::vector::plan::VerifiedVectorPlan;
use crate::signal_fir::vector::verify::{ValueType, VectorPlan, VectorPlanError};
use std::collections::BTreeSet;
use std::fmt;

/// Current canonical state-plan schema.
pub const VECTOR_STATE_PLAN_VERSION: u32 = 4;
/// One `CodeLoop` execution phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VectorStatePhase {
    /// Runs once before the loop body: state is staged into working storage.
    Pre,
    /// Runs inside the per-sample loop body: state is read and written.
    Exec,
    /// Runs once after the loop body: state is persisted for the next chunk.
    Post,
}
/// Exact vector-mode storage selected for one delayed carrier.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VectorDelayStorage {
    /// Delay-one state carried in a scalar local for one certified lockstep
    /// lane. Eligibility is bound by [`LockstepRegisterBundle`] rather than
    /// inferred from these generated names.
    Register {
        /// Scalar local carrying the lane value inside the loop body.
        local_name: String,
        /// Persistent struct field the local is loaded from and stored to.
        persistent_name: String,
        /// Identifier of the owning [`LockstepRegisterBundle`].
        bundle_id: u64,
        /// Ordered lane position within the owning bundle.
        lane: u64,
    },
    /// C++ `_tmp`/`_perm` dual-buffer representation.
    Copy {
        /// Working `_tmp` buffer written during the chunk.
        temporary_name: String,
        /// Persistent `_perm` buffer holding history across chunks.
        permanent_name: String,
        /// Number of history samples copied in before and out after a chunk.
        history_length: u64,
        /// Total working-buffer length (`vec_size + history_length`).
        temporary_length: u64,
    },
    /// C++ power-of-two ring representation.
    Ring {
        /// Persistent power-of-two circular buffer.
        buffer_name: String,
        /// Working write index advanced inside the loop body.
        index_name: String,
        /// Persistent saved index restored before and stored after a chunk.
        index_save_name: String,
        /// Power-of-two buffer length.
        capacity: u64,
        /// Wrap mask, always `capacity - 1`.
        mask: u64,
    },
    /// Persistent ring indexed by one shared clock-domain fire-time cursor.
    ClockRing {
        /// Persistent power-of-two circular buffer.
        buffer_name: String,
        /// Shared fire-time cursor owned by the clock domain.
        cursor_name: String,
        /// Clock domain whose cursor indexes this ring.
        domain_id: u64,
        /// Power-of-two buffer length.
        capacity: u64,
        /// Wrap mask, always `capacity - 1`.
        mask: u64,
    },
}
/// Canonical phase operation. Enum order is also canonical operation order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VectorStateAction {
    /// Load register-carried delay-one state from its persistent field.
    DelayRegisterLoad {
        /// Delayed signal whose register state is loaded.
        signal_id: u64,
    },
    /// Copy persistent history into the working `_tmp` buffer.
    DelayCopyIn {
        /// Delayed signal whose history is staged.
        signal_id: u64,
    },
    /// Restore the ring write index from its persistent saved copy.
    DelayRingAdvance {
        /// Delayed signal whose ring index is restored.
        signal_id: u64,
    },
    /// Execute one simultaneous step of a symbolic recursion group.
    RecursionStep {
        /// Recursion group advanced by this step.
        group: u64,
    },
    /// Write the current sample into the signal's delay storage.
    DelayWrite {
        /// Delayed signal whose current value is written.
        signal_id: u64,
    },
    /// Update the `prefix` previous-sample cell with the current value.
    PrefixWrite {
        /// Prefix signal whose state cell is updated.
        signal_id: u64,
    },
    /// Advance the cycling waveform read index, wrapping at the length.
    WaveformAdvance {
        /// Waveform signal whose index is advanced.
        signal_id: u64,
    },
    /// Store register-carried delay-one state back to its persistent field.
    DelayRegisterStore {
        /// Delayed signal whose register state is stored.
        signal_id: u64,
    },
    /// Copy the working `_tmp` tail back into persistent history.
    DelayCopyOut {
        /// Delayed signal whose history is persisted.
        signal_id: u64,
    },
    /// Persist the advanced ring write index into its saved copy.
    DelayRingSaveAdvance {
        /// Delayed signal whose ring index is saved.
        signal_id: u64,
    },
}
impl VectorStateAction {
    /// Phase in which this action must execute.
    #[must_use]
    pub fn phase(&self) -> VectorStatePhase {
        match self {
            Self::DelayRegisterLoad { .. }
            | Self::DelayCopyIn { .. }
            | Self::DelayRingAdvance { .. } => VectorStatePhase::Pre,
            Self::RecursionStep { .. }
            | Self::DelayWrite { .. }
            | Self::PrefixWrite { .. }
            | Self::WaveformAdvance { .. } => VectorStatePhase::Exec,
            Self::DelayRegisterStore { .. }
            | Self::DelayCopyOut { .. }
            | Self::DelayRingSaveAdvance { .. } => VectorStatePhase::Post,
        }
    }
}
/// Storage transition for one signal whose certified `max_delay` is nonzero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelayTransition {
    /// Delayed signal this transition manages state for.
    pub signal_id: u64,
    /// Vector-plan loop that owns the signal's computation.
    pub loop_id: u64,
    /// Scalar element type stored in the delay line.
    pub value_type: ValueType,
    /// Certified maximum delay in samples read from this signal.
    pub max_delay: u64,
    /// `None` for top-rate chunk time, `Some(d)` for domain fire time.
    pub clock_domain: Option<u64>,
    /// Exact storage representation selected for this delay line.
    pub storage: VectorDelayStorage,
}
/// Canonical lifecycle initializer for one scalar state cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VectorStateInitialValue {
    /// Exact integer initializer.
    Int(i32),
    /// Real initializer carried as raw bits for exact equality and hashing.
    RealBits(u64),
    /// Type-appropriate zero initializer.
    Zero,
}
/// One `prefix(init, value)` previous-sample state cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrefixTransition {
    /// Prefix signal this transition manages state for.
    pub signal_id: u64,
    /// Vector-plan loop that owns the signal's computation.
    pub loop_id: u64,
    /// Signal producing the value stored for the next sample.
    pub value_signal_id: u64,
    /// Persistent scalar cell holding the previous-sample value.
    pub state_name: String,
    /// Scalar type of the state cell.
    pub value_type: ValueType,
    /// Value the cell holds before the first sample is produced.
    pub initial: VectorStateInitialValue,
}
/// One cycling direct waveform read index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveformTransition {
    /// Waveform signal this transition manages state for.
    pub signal_id: u64,
    /// Vector-plan loop that owns the signal's computation.
    pub loop_id: u64,
    /// Persistent cycling read-index cell.
    pub index_name: String,
    /// Waveform table length the index wraps at.
    pub length: u64,
    /// Scalar element type of the waveform samples.
    pub value_type: ValueType,
}
/// One projection participating in a simultaneous recursion step.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecursionProjectionTransition {
    /// Projection position within the recursion group.
    pub index: u64,
    /// Prepared signal aliases that read this one symbolic projection.
    pub signal_ids: Vec<u64>,
    /// Signal computing the next value. This is the first visible projection
    /// alias when one exists, otherwise the recursive body itself.
    pub value_signal_id: u64,
}
/// One symbolic recursion group and its serial owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecursionTransition {
    /// Symbolic recursion group this transition steps.
    pub group: u64,
    /// Recursive vector-plan loop that serially owns the group.
    pub loop_id: u64,
    /// Projections advanced simultaneously, in canonical index order.
    pub projections: Vec<RecursionProjectionTransition>,
}
/// One ordered register-carried lane in a certified lockstep bundle.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LockstepRegisterLane {
    /// Ordered lane position within the bundle.
    pub lane: u64,
    /// Vector-plan loop that owns the lane's signal.
    pub loop_id: u64,
    /// Recursion group whose lockstep certification admits this lane.
    pub recursion_group: u64,
    /// Delayed signal carried in this lane.
    pub signal_id: u64,
    /// Scalar local carrying the lane value inside the loop body.
    pub local_name: String,
    /// Persistent struct field the local is loaded from and stored to.
    pub persistent_name: String,
}
/// Checked bundle-level identity for register-carried delay-one state.
///
/// This record is an adapted Rust representation of the scalar state carried
/// by Faust C++ `CodeLoop` execution. It prevents assembly from deciding
/// eligibility from variable names and keeps lane order, state identity, and
/// persistent boundaries co-located.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LockstepRegisterBundle {
    /// Stable identifier referenced by [`VectorDelayStorage::Register`].
    pub bundle_id: u64,
    /// Certified lanes in canonical lane order.
    pub lanes: Vec<LockstepRegisterLane>,
}
/// Complete phase bodies for one stateful vector-plan loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopStatePhases {
    /// Vector-plan loop these phase bodies belong to.
    pub loop_id: u64,
    /// Actions executed once before the loop body, in canonical order.
    pub pre: Vec<VectorStateAction>,
    /// Actions executed inside the loop body, in canonical order.
    pub exec: Vec<VectorStateAction>,
    /// Actions executed once after the loop body, in canonical order.
    pub post: Vec<VectorStateAction>,
}
/// Canonical finite state-transition artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorStatePlan {
    /// Schema stamp; must equal [`VECTOR_STATE_PLAN_VERSION`].
    pub schema_version: u32,
    /// Vector chunk size the plan was built for.
    pub vec_size: u64,
    /// Threshold at or below which copy storage is selected over a ring.
    pub max_copy_delay: u64,
    /// Phase bodies for every stateful loop, in canonical loop order.
    pub loops: Vec<LoopStatePhases>,
    /// Delay-line transitions, in canonical signal order.
    pub delays: Vec<DelayTransition>,
    /// Recursion-group transitions, in canonical group order.
    pub recursions: Vec<RecursionTransition>,
    /// Certified register bundles referenced by register delay storage.
    pub lockstep_register_bundles: Vec<LockstepRegisterBundle>,
    /// Prefix previous-sample transitions, in canonical signal order.
    pub prefixes: Vec<PrefixTransition>,
    /// Cycling waveform-index transitions, in canonical signal order.
    pub waveforms: Vec<WaveformTransition>,
    /// Conservative delay effects proven to have no positive-delay use.
    /// They require no storage or runtime action (`x @ 0 == x`).
    pub no_op_resources: Vec<StateResource>,
}
/// Opaque evidence that state-plan construction passed its independent checker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedVectorStatePlan {
    pub(super) plan: VectorStatePlan,
    pub(super) vector_plan: VectorPlan,
    pub(super) delegated_resources: BTreeSet<StateResource>,
}
impl VerifiedVectorStatePlan {
    /// Checked state plan carried by this evidence.
    #[must_use]
    pub fn plan(&self) -> &VectorStatePlan {
        &self.plan
    }

    /// Vector plan the state plan was verified against.
    #[must_use]
    pub fn vector_plan(&self) -> &VectorPlan {
        &self.vector_plan
    }

    /// Consumes the evidence, yielding the checked state plan.
    #[must_use]
    pub fn into_plan(self) -> VectorStatePlan {
        self.plan
    }

    /// State resources whose generic event-certificate effects are refined by this plan.
    #[must_use]
    pub fn managed_resources(&self) -> BTreeSet<StateResource> {
        let mut resources = managed_resources(&self.plan);
        resources.extend(self.delegated_resources.iter().cloned());
        resources
    }
}
#[cfg(test)]
pub(crate) fn verified_vector_state_plan_for_test(
    plan: VectorStatePlan,
    vector_plan: &VerifiedVectorPlan,
) -> VerifiedVectorStatePlan {
    verify_phases(&plan).expect("test vector state plan must have canonical phases");
    VerifiedVectorStatePlan {
        plan,
        vector_plan: vector_plan.plan().clone(),
        delegated_resources: BTreeSet::new(),
    }
}
/// Typed producer/checker failure at the state-plan boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VectorStateError {
    /// Verification of the underlying vector plan itself failed.
    Plan(VectorPlanError),
    /// The plan's schema stamp is not [`VECTOR_STATE_PLAN_VERSION`].
    UnsupportedSchema {
        /// Schema version found in the plan.
        found: u32,
    },
    /// The plan's declared vector size disagrees with the vector plan.
    VecSizeMismatch {
        /// Vector size declared by the state plan.
        declared: u64,
        /// Vector size carried by the verified vector plan.
        actual: u64,
    },
    /// Source facts (signals, types, loops) do not cover a referenced signal.
    SignalCoverageMismatch {
        /// Signal missing from the source facts.
        signal_id: u64,
    },
    /// Independently derived source facts disagree for one signal.
    SignalFactMismatch {
        /// Signal whose facts disagree.
        signal_id: u64,
    },
    /// A stateful signal is not owned by any vector-plan loop.
    MissingLoopOwner {
        /// Stateful signal without an owning loop.
        signal_id: u64,
    },
    /// A delayed signal is owned by a loop that is not a vector loop.
    DelayOwnerNotVectorLoop {
        /// Delayed signal with the invalid owner.
        signal_id: u64,
        /// Non-vector loop claiming ownership.
        loop_id: u64,
    },
    /// A recursion group is not owned by the expected recursive loop.
    RecursionLoopMismatch {
        /// Recursion group with the invalid owner.
        group: u64,
        /// Loop that fails to serially own the group.
        loop_id: u64,
    },
    /// Clocked state was encountered without clock/AD-plan support in scope.
    UnsupportedClockState {
        /// Signal carrying the clocked state.
        signal_id: u64,
        /// Clock domain the state belongs to.
        clock_id: u32,
    },
    /// Clocked state was encountered without a checked clock/AD plan.
    ClockPlanRequired {
        /// Signal carrying the clocked state.
        signal_id: u64,
        /// Clock domain lacking a checked plan.
        clock_id: u32,
    },
    /// A clocked state signal is owned by a loop outside its clock domain.
    ClockLoopMismatch {
        /// Clocked state signal with the invalid owner.
        signal_id: u64,
        /// Clock domain the signal belongs to.
        clock_id: u64,
        /// Owning loop that lies outside the domain.
        loop_id: u64,
    },
    /// A state resource has no transition model in this stage.
    UnsupportedStateResource {
        /// Resource requiring a later transition model.
        resource: StateResource,
    },
    /// Delay geometry arithmetic overflowed while sizing storage.
    ArithmeticOverflow {
        /// Signal whose geometry overflowed.
        signal_id: u64,
    },
    /// A plan sequence is not sorted, deduplicated, and canonically ordered.
    NotCanonical {
        /// Name of the sequence that violates canonical order.
        what: &'static str,
        /// Index of the first element that violates canonical order.
        at: usize,
    },
    /// Delay transitions do not match the certified delay facts one-to-one.
    DelayCoverageMismatch,
    /// Recursion transitions do not match the recursion facts one-to-one.
    RecursionCoverageMismatch,
    /// A loop's pre/exec/post bodies disagree with the required actions.
    LoopPhaseMismatch {
        /// Loop whose phase bodies are invalid.
        loop_id: u64,
    },
    /// Simulated delay storage geometry disagrees with the transition.
    SimulationGeometryMismatch,
    /// Simulation read a delay beyond the certified maximum.
    SimulationDelayOutOfRange {
        /// Delay requested by the simulation.
        delay: usize,
        /// Certified maximum delay.
        max_delay: usize,
    },
    /// Simulation was driven with a chunk longer than the vector size.
    SimulationChunkTooLarge {
        /// Chunk length requested.
        count: usize,
        /// Configured vector size.
        vec_size: usize,
    },
    /// A recursion step produced a tuple whose arity differs from the state.
    RecursionArityMismatch {
        /// Arity of the current state tuple.
        state: usize,
        /// Arity produced by the step.
        next: usize,
    },
}
impl fmt::Display for VectorStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan(error) => write!(f, "vector plan verification failed: {error}"),
            Self::UnsupportedSchema { found } => {
                write!(f, "unsupported vector-state schema {found}")
            }
            Self::VecSizeMismatch { declared, actual } => write!(
                f,
                "state-plan vector size {declared} differs from vector plan {actual}"
            ),
            Self::SignalCoverageMismatch { signal_id } => {
                write!(f, "state-plan source facts do not cover signal {signal_id}")
            }
            Self::SignalFactMismatch { signal_id } => {
                write!(f, "state-plan source facts disagree for signal {signal_id}")
            }
            Self::MissingLoopOwner { signal_id } => {
                write!(f, "stateful signal {signal_id} has no owned loop")
            }
            Self::DelayOwnerNotVectorLoop { signal_id, loop_id } => write!(
                f,
                "delayed signal {signal_id} is owned by non-vector loop {loop_id}"
            ),
            Self::RecursionLoopMismatch { group, loop_id } => write!(
                f,
                "recursion group {group} is not owned by recursive loop {loop_id}"
            ),
            Self::UnsupportedClockState {
                signal_id,
                clock_id,
            } => write!(
                f,
                "clocked state on signal {signal_id} (domain {clock_id}) requires P6.2"
            ),
            Self::ClockPlanRequired {
                signal_id,
                clock_id,
            } => write!(
                f,
                "clocked state on signal {signal_id} (domain {clock_id}) requires a checked P6.2 plan"
            ),
            Self::ClockLoopMismatch {
                signal_id,
                clock_id,
                loop_id,
            } => write!(
                f,
                "clocked state signal {signal_id} is owned by loop {loop_id}, outside domain {clock_id}"
            ),
            Self::UnsupportedStateResource { resource } => {
                write!(
                    f,
                    "state resource {resource:?} requires a later P6 transition model"
                )
            }
            Self::ArithmeticOverflow { signal_id } => {
                write!(f, "delay geometry overflow for signal {signal_id}")
            }
            Self::NotCanonical { what, at } => {
                write!(f, "{what} is not canonical at index {at}")
            }
            Self::DelayCoverageMismatch => write!(f, "delay transitions do not cover source facts"),
            Self::RecursionCoverageMismatch => {
                write!(f, "recursion transitions do not cover source facts")
            }
            Self::LoopPhaseMismatch { loop_id } => {
                write!(f, "pre/exec/post actions are invalid for loop {loop_id}")
            }
            Self::SimulationGeometryMismatch => write!(f, "delay simulation geometry mismatch"),
            Self::SimulationDelayOutOfRange { delay, max_delay } => {
                write!(f, "delay {delay} exceeds certified maximum {max_delay}")
            }
            Self::SimulationChunkTooLarge { count, vec_size } => write!(
                f,
                "chunk length {count} exceeds configured vector size {vec_size}"
            ),
            Self::RecursionArityMismatch { state, next } => write!(
                f,
                "recursion step has arity {next}, expected current state arity {state}"
            ),
        }
    }
}
impl std::error::Error for VectorStateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Plan(error) => Some(error),
            _ => None,
        }
    }
}
impl From<VectorPlanError> for VectorStateError {
    fn from(value: VectorPlanError) -> Self {
        Self::Plan(value)
    }
}
