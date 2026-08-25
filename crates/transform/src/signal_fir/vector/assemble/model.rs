//! Assembly artifact DTOs, the verified wrapper, the error taxonomy,
//! and total helper conversions shared by producer and checker
//! (plan provenance: §4.6: vocabulary and total conversions are shareable).

use crate::signal_fir::vector::clock_ad::{ClockGuard, ClockIsland};
use crate::signal_fir::vector::route::VerifiedRoutedFir;
use crate::signal_fir::vector::state::VectorStateAction;
use crate::signal_fir::vector::verify::VectorPlan;
use fir::FirId;
use std::collections::BTreeSet;
use std::fmt;
/// Current canonical assembly schema.
pub const VECTOR_FIR_ASSEMBLY_VERSION: u32 = 3;
/// Already-lowered non-state statements owned by one checked planned loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorLoopFirInput {
    /// Stable loop id from the vector plan.
    pub loop_id: u64,
    /// Lowered loop-body statements in emission order.
    pub statements: Vec<FirId>,
}
/// One top-rate output store whose value is produced by a held clock island.
///
/// This is an adapted Rust representation of the C++ post-island output write:
/// ownership is explicit rather than inferred from a mutable `CodeLoop` tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorClockOutputStore {
    /// Loop that owns the top-rate output store.
    pub owner_loop_id: u64,
    /// FIR store writing the held island value to the output.
    pub statement: FirId,
}
/// Concrete statement implementing one accepted state-plan action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorStateFirAction {
    /// Accepted state-plan action being implemented.
    pub action: VectorStateAction,
    /// FIR statement implementing the action.
    pub statement: FirId,
    /// Statements inserted into the enclosing sample scope. Recursion-step
    /// declarations are flattened so subsequent delay writes can read them.
    pub execution_statements: Vec<FirId>,
}
/// One loop body after state-phase materialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssembledVectorLoop {
    /// Stable loop id from the vector plan.
    pub loop_id: u64,
    /// State actions executed once before the sample loop.
    pub pre: Vec<VectorStateFirAction>,
    /// Loop-body statements executed on every sample iteration.
    pub exec: Vec<FirId>,
    /// State actions whose execution statements run inside the sample loop.
    pub exec_actions: Vec<VectorStateFirAction>,
    /// State actions executed once after the sample loop.
    pub post: Vec<VectorStateFirAction>,
    /// Complete outer-chunk execution: `pre; for i0; post`.
    pub chunk_statement: FirId,
    /// One serial iteration used when this loop is nested below a clock guard.
    pub iteration_statement: FirId,
}
/// One nested serial clock domain after guard construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssembledClockIsland {
    /// Stable clock-domain id from the clock/AD plan.
    pub domain_id: u64,
    /// Enclosing clock domain, if this island is nested.
    pub parent_domain: Option<u64>,
    /// Clock guard controlling when the island body executes.
    pub guard: ClockGuard,
    /// Scheduled loops executed serially inside this island.
    pub nested_loop_ids: Vec<u64>,
    /// Clock-plan `IslandScalar` declarations whose lifetime begins below this guard.
    pub local_declarations: Vec<FirId>,
    /// Optional shared state-cursor advance inside this domain guard.
    pub state_cursor_advance: Option<FirId>,
    /// Complete guarded FIR statement for the island.
    pub statement: FirId,
}
/// Finite FIR assembly accepted before final module placement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorFirAssembly {
    /// Schema version, always [`VECTOR_FIR_ASSEMBLY_VERSION`].
    pub schema_version: u32,
    /// Stack-scoped declarations emitted before the loops.
    pub local_declarations: Vec<FirId>,
    /// Persistent struct-state declarations.
    pub state_declarations: Vec<FirId>,
    /// Statements resetting persistent state in the clear path.
    pub clear_statements: Vec<FirId>,
    /// Assembled loops in scheduled execution order.
    pub loops: Vec<AssembledVectorLoop>,
    /// Assembled clock islands in scheduled execution order.
    pub islands: Vec<AssembledClockIsland>,
    /// Top-rate output stores fed by held clock islands.
    pub clock_output_stores: Vec<VectorClockOutputStore>,
    /// Root FIR block containing the whole assembly.
    pub top_level_statement: FirId,
}
/// Opaque evidence that the assembly checker accepted an assembly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedVectorFirAssembly {
    pub(super) assembly: VectorFirAssembly,
    pub(super) vector_plan: VectorPlan,
}
impl VerifiedVectorFirAssembly {
    /// Returns the accepted assembly.
    #[must_use]
    pub fn assembly(&self) -> &VectorFirAssembly {
        &self.assembly
    }

    /// Returns the verified plan the assembly was checked against.
    #[must_use]
    pub fn vector_plan(&self) -> &VectorPlan {
        &self.vector_plan
    }

    /// Consumes the evidence and returns the accepted assembly.
    #[must_use]
    pub fn into_assembly(self) -> VectorFirAssembly {
        self.assembly
    }
}
/// Typed producer/checker failure at the assembly boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum VectorFirAssemblyError {
    /// An input artifact does not belong to the routed vector plan.
    PlanMismatch {
        /// Name of the mismatched artifact.
        artifact: &'static str,
    },
    /// A signal requires the certified scalar reverse-AD window.
    ReverseAdRequiresScalar {
        /// Stable signal id.
        signal_id: u64,
    },
    /// Loop FIR inputs do not cover the planned loops exactly.
    LoopInputCoverage {
        /// Stable loop id.
        loop_id: u64,
    },
    /// The same loop was provided as a FIR input more than once.
    DuplicateLoopInput {
        /// Stable loop id.
        loop_id: u64,
    },
    /// A stateful signal has no routed definition in its owning loop.
    MissingDefinition {
        /// Stable signal id.
        signal_id: u64,
        /// Stable loop id.
        loop_id: u64,
    },
    /// A recursion-group projection has no routed definition.
    MissingRecursionProjection {
        /// Recursion group id.
        group: u64,
        /// Projection index within the group.
        index: u64,
    },
    /// Assembled state actions disagree with the accepted state-plan phases.
    LoopStateCoverage {
        /// Stable loop id.
        loop_id: u64,
    },
    /// A loop is claimed by more than one clock island.
    ClockLoopOwnership {
        /// Stable loop id.
        loop_id: u64,
    },
    /// A clock island cannot resolve one of its clock signals.
    MissingClockValue {
        /// Stable clock-domain id.
        domain_id: u64,
        /// Unresolved clock signal id.
        signal_id: u64,
    },
    /// A clock island references a parent domain that does not exist.
    MissingClockParent {
        /// Stable clock-domain id.
        domain_id: u64,
        /// Missing parent domain id.
        parent_id: u64,
    },
    /// A value does not fit FIR i32 arithmetic.
    ArithmeticOverflow {
        /// Name of the overflowing quantity.
        what: &'static str,
        /// Offending value.
        value: u64,
    },
    /// A signal has tuple-valued state storage.
    UnsupportedValueType {
        /// Stable signal id.
        signal_id: u64,
    },
    /// An assembled declaration has an invalid FIR shape.
    DeclarationShape {
        /// Declaration name.
        name: String,
    },
    /// A loop state action produced invalid FIR.
    ActionShape {
        /// Stable loop id.
        loop_id: u64,
        /// Offending state action.
        action: VectorStateAction,
    },
    /// A clock island has an invalid FIR guard.
    IslandShape {
        /// Stable clock-domain id.
        domain_id: u64,
    },
    /// A fused serial group has an invalid FIR shape.
    FusedGroupShape {
        /// Stable fused group id.
        group_id: u64,
    },
    /// A verified lockstep bundle was not emitted as one physical sample loop.
    LockstepBundleShape {
        /// Stable lockstep bundle id.
        bundle_id: u64,
    },
    /// The assembled top-level FIR is not a block.
    TopLevelShape,
}
impl fmt::Display for VectorFirAssemblyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlanMismatch { artifact } => {
                write!(f, "{artifact} does not belong to the routed vector plan")
            }
            Self::ReverseAdRequiresScalar { signal_id } => write!(
                f,
                "signal {signal_id} requires the certified scalar reverse-AD window"
            ),
            Self::LoopInputCoverage { loop_id } => {
                write!(f, "loop FIR inputs do not cover loop {loop_id} exactly")
            }
            Self::DuplicateLoopInput { loop_id } => {
                write!(f, "loop FIR input {loop_id} is duplicated")
            }
            Self::MissingDefinition { signal_id, loop_id } => write!(
                f,
                "stateful signal {signal_id} has no routed definition in loop {loop_id}"
            ),
            Self::MissingRecursionProjection { group, index } => write!(
                f,
                "recursion group {group} projection {index} has no routed definition"
            ),
            Self::LoopStateCoverage { loop_id } => {
                write!(f, "assembled state actions disagree for loop {loop_id}")
            }
            Self::ClockLoopOwnership { loop_id } => {
                write!(f, "loop {loop_id} belongs to more than one clock island")
            }
            Self::MissingClockValue {
                domain_id,
                signal_id,
            } => write!(
                f,
                "clock island {domain_id} cannot resolve clock signal {signal_id}"
            ),
            Self::MissingClockParent {
                domain_id,
                parent_id,
            } => write!(
                f,
                "clock island {domain_id} references missing parent {parent_id}"
            ),
            Self::ArithmeticOverflow { what, value } => {
                write!(f, "{what} value {value} does not fit FIR i32 arithmetic")
            }
            Self::UnsupportedValueType { signal_id } => {
                write!(f, "signal {signal_id} has tuple-valued state storage")
            }
            Self::DeclarationShape { name } => {
                write!(f, "assembled declaration {name} has an invalid FIR shape")
            }
            Self::ActionShape { loop_id, action } => {
                write!(f, "loop {loop_id} action {action:?} has invalid FIR")
            }
            Self::IslandShape { domain_id } => {
                write!(f, "clock island {domain_id} has an invalid FIR guard")
            }
            Self::FusedGroupShape { group_id } => {
                write!(f, "fused serial group {group_id} has an invalid FIR shape")
            }
            Self::LockstepBundleShape { bundle_id } => {
                write!(f, "lockstep bundle {bundle_id} has an invalid FIR shape")
            }
            Self::TopLevelShape => write!(f, "assembled top-level FIR is not a block"),
        }
    }
}
impl std::error::Error for VectorFirAssemblyError {}
pub(super) fn scheduled_island_loop_ids(
    routed: &VerifiedRoutedFir,
    island: &ClockIsland,
) -> Vec<u64> {
    let members = island
        .nested_loop_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    routed
        .layout()
        .loops()
        .iter()
        .filter_map(|region| members.contains(&region.loop_id).then_some(region.loop_id))
        .collect()
}
pub(super) fn recursion_name(group: u64, index: u64) -> String {
    format!("vrec_g{group}_p{index}_next")
}
