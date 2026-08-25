//! Strategy-dependent vector execution schedules over [`VectorPlan`].
//!
//! Production scheduling stage (history: P5 slice of
//! `vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`): the
//! strategy-independent plan is verified first, then each epoch's induced DAG
//! is serialized in ascending rank. The plan certificate remains unchanged
//! when the scheduling strategy changes.
//!
//! The implementation delegates every ordering decision to the generic
//! [`crate::schedule::schedule`] adapter, which is the Rust port of the C++
//! `dfschedule`, `bfschedule`, `spschedule`, and `rbschedule` functions in
//! `compiler/DirectedGraph/Schedule.hh`. Same-epoch data and effect edges use
//! the C++ dependency convention `consumer -> dependency`; cross-epoch edges
//! are barriers validated by [`verify_vector_plan`] and are intentionally
//! absent from each local DAG.
//!
//! A checked fused serial group is contracted to one external scheduling unit
//! so dependencies that become internal cannot create an artificial cycle.
//! Its induced member DAG is still scheduled independently with the requested
//! strategy and checked before expansion, preserving both the group envelope
//! and the original loop identities.

use std::fmt;

use crate::schedule::{
    ScheduleDag, ScheduleError, SchedulingStrategy, VerifyError, schedule, verify_schedule,
};

use super::plan::VerifiedVectorPlan;
use super::verify::{EpochRecord, VectorPlan, VectorPlanError, verify_vector_plan};

/// The selected order for one verified vector-plan epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledEpoch {
    /// The strategy-independent epoch identity.
    pub epoch_id: u64,
    /// The fixed execution rank of this epoch.
    pub rank: u64,
    /// Complete topological order of this epoch's loop IDs.
    pub loops: Vec<u64>,
}

/// A strategy-dependent execution schedule over a strategy-independent
/// [`VectorPlan`]. Epoch membership, IDs, transports, and edges remain owned
/// by the input plan; only the per-epoch loop order depends on `strategy`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorExecutionSchedule {
    /// The normalized scheduling policy used to produce `epochs`.
    pub strategy: SchedulingStrategy,
    /// Epoch schedules in ascending rank, independently of epoch IDs.
    pub epochs: Vec<ScheduledEpoch>,
}

/// Why [`schedule_vector_plan`] could not produce a complete execution
/// schedule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VectorScheduleError {
    /// The strategy-independent plan failed its certificate verification gate.
    PlanVerification(VectorPlanError),
    /// The generic scheduler rejected an induced epoch DAG.
    EpochScheduling {
        /// The epoch whose induced DAG failed.
        epoch_id: u64,
        /// The typed generic scheduler failure.
        source: ScheduleError<u64>,
    },
    /// The independent schedule postcondition checker rejected an order
    /// returned by the generic scheduler.
    Postcondition {
        /// The epoch whose candidate order was rejected.
        epoch_id: u64,
        /// The independently detected soundness/completeness failure.
        source: VerifyError<u64>,
    },
}

impl fmt::Display for VectorScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlanVerification(error) => write!(f, "vector plan verification failed: {error}"),
            Self::EpochScheduling { epoch_id, source } => {
                write!(f, "scheduling epoch {epoch_id} failed: {source}")
            }
            Self::Postcondition { epoch_id, source } => {
                write!(
                    f,
                    "schedule postcondition failed for epoch {epoch_id}: {source}"
                )
            }
        }
    }
}

impl std::error::Error for VectorScheduleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PlanVerification(error) => Some(error),
            Self::EpochScheduling { source, .. } => Some(source),
            Self::Postcondition { source, .. } => Some(source),
        }
    }
}

/// Schedules every epoch of `plan` with the selected generic scheduling
/// strategy.
///
/// Verification is deliberately the first operation. Once it succeeds,
/// cross-epoch edges are treated only as already-checked rank barriers and do
/// not enter any epoch DAG. The input is borrowed read-only, so changing
/// `strategy` cannot change the [`VectorPlan`] structurally (certificate invariant).
///
/// # Errors
/// Returns [`VectorScheduleError::PlanVerification`] for the first failed certificate
/// plan obligation, or [`VectorScheduleError::EpochScheduling`] with the
/// original generic scheduler error and epoch ID. A scheduler result that
/// fails the independent checker returns [`VectorScheduleError::Postcondition`].
pub fn schedule_vector_plan(
    plan: &VectorPlan,
    strategy: SchedulingStrategy,
) -> Result<VectorExecutionSchedule, VectorScheduleError> {
    verify_vector_plan(plan).map_err(VectorScheduleError::PlanVerification)?;
    schedule_after_plan_verification(plan, strategy)
}

/// Schedules a plan whose opaque producer boundary has already run the full
/// independent plan checker. This avoids repeating the expensive global plan
/// verification in downstream routing while preserving the checked public
/// [`schedule_vector_plan`] entry point for raw plans.
pub(crate) fn schedule_verified_vector_plan(
    verified: &VerifiedVectorPlan,
    strategy: SchedulingStrategy,
) -> Result<VectorExecutionSchedule, VectorScheduleError> {
    schedule_after_plan_verification(verified.plan(), strategy)
}

fn schedule_after_plan_verification(
    plan: &VectorPlan,
    strategy: SchedulingStrategy,
) -> Result<VectorExecutionSchedule, VectorScheduleError> {
    let mut epochs: Vec<&EpochRecord> = plan.epochs.iter().collect();
    epochs.sort_unstable_by_key(|epoch| (epoch.rank, epoch.epoch_id));

    let scheduled_epochs = epochs
        .into_iter()
        .map(|epoch| {
            let dag = EpochDag::new(&epoch.loops, plan, strategy).map_err(|source| {
                VectorScheduleError::EpochScheduling {
                    epoch_id: epoch.epoch_id,
                    source,
                }
            })?;
            let units = schedule(strategy, &dag).map_err(|source| {
                VectorScheduleError::EpochScheduling {
                    epoch_id: epoch.epoch_id,
                    source,
                }
            })?;
            verify_schedule(&dag, &units).map_err(|source| VectorScheduleError::Postcondition {
                epoch_id: epoch.epoch_id,
                source,
            })?;
            let loops = dag.expand_lockstep_units(&units);
            Ok(ScheduledEpoch {
                epoch_id: epoch.epoch_id,
                rank: epoch.rank,
                loops,
            })
        })
        .collect::<Result<Vec<_>, VectorScheduleError>>()?;

    Ok(VectorExecutionSchedule {
        strategy,
        epochs: scheduled_epochs,
    })
}

/// `ScheduleDag` view of one epoch.
///
/// Filtering both endpoints keeps cross-epoch barriers out of local
/// scheduling while preserving all same-epoch data/effect constraints.
/// Lockstep bundles and fused serial groups are represented as contracted
/// units; fused members retain a separately checked internal schedule.
struct EpochDag<'a> {
    nodes: Vec<u64>,
    dependencies: std::collections::BTreeMap<u64, Vec<u64>>,
    unit_members: std::collections::BTreeMap<u64, Vec<u64>>,
    marker: std::marker::PhantomData<&'a VectorPlan>,
}

impl<'a> EpochDag<'a> {
    fn new(
        nodes: &'a [u64],
        plan: &'a VectorPlan,
        strategy: SchedulingStrategy,
    ) -> Result<Self, ScheduleError<u64>> {
        use std::collections::{BTreeMap, BTreeSet};

        let mut representative = plan
            .lockstep_bundles
            .iter()
            .flat_map(|bundle| {
                bundle
                    .member_loop_ids
                    .iter()
                    .map(move |&loop_id| (loop_id, bundle.representative_loop_id))
            })
            .collect::<BTreeMap<_, _>>();
        let mut unit_members = plan
            .lockstep_bundles
            .iter()
            .map(|bundle| {
                (
                    bundle.representative_loop_id,
                    bundle.member_loop_ids.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for group in &plan.fused_serial_groups {
            let group_representative = group.member_loop_ids[0];
            for &loop_id in &group.member_loop_ids {
                representative.insert(loop_id, group_representative);
            }
            let group_dag = InducedDag::new(&group.member_loop_ids, plan);
            let members = schedule(strategy, &group_dag).map_err(|error| {
                if std::env::var_os("FAUST_RS_VECTOR_TIMING").is_some() {
                    eprintln!(
                        "[vector-fused-internal-schedule] group={} error={} dependencies={:?}",
                        group.group_id, error, group_dag.dependencies
                    );
                }
                error
            })?;
            verify_schedule(&group_dag, &members).map_err(|error| {
                let remaining = match error {
                    VerifyError::DuplicateGraphNode { node }
                    | VerifyError::Duplicate { node }
                    | VerifyError::Missing { node }
                    | VerifyError::Extra { node } => vec![node],
                    VerifyError::OutOfOrder {
                        consumer,
                        dependency,
                    } => vec![consumer, dependency],
                };
                ScheduleError::Cycle { remaining }
            })?;
            unit_members.insert(group_representative, members);
        }
        let unit_of = |loop_id: u64| representative.get(&loop_id).copied().unwrap_or(loop_id);
        let unit_nodes = nodes
            .iter()
            .copied()
            .filter(|loop_id| unit_of(*loop_id) == *loop_id)
            .collect::<Vec<_>>();
        let node_set = nodes.iter().copied().collect::<BTreeSet<_>>();
        let mut direct = unit_nodes
            .iter()
            .copied()
            .map(|node| (node, BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();

        for edge in plan.data_edges.iter().chain(&plan.effect_edges) {
            if !node_set.contains(&edge.consumer) || !node_set.contains(&edge.dependency) {
                continue;
            }
            let consumer = unit_of(edge.consumer);
            let dependency = unit_of(edge.dependency);
            if consumer == dependency {
                continue;
            }
            direct
                .get_mut(&consumer)
                .expect("epoch nodes initialize direct dependencies")
                .insert(dependency);
        }

        let dependencies = unit_nodes
            .iter()
            .copied()
            .map(|node| {
                (
                    node,
                    direct
                        .remove(&node)
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                )
            })
            .collect();

        Ok(Self {
            nodes: unit_nodes,
            dependencies,
            unit_members,
            marker: std::marker::PhantomData,
        })
    }

    fn expand_lockstep_units(&self, units: &[u64]) -> Vec<u64> {
        units
            .iter()
            .flat_map(|unit| {
                self.unit_members
                    .get(unit)
                    .map(Vec::as_slice)
                    .unwrap_or(std::slice::from_ref(unit))
                    .iter()
                    .copied()
            })
            .collect()
    }
}

struct InducedDag {
    nodes: Vec<u64>,
    dependencies: std::collections::BTreeMap<u64, Vec<u64>>,
}

impl InducedDag {
    fn new(nodes: &[u64], plan: &VectorPlan) -> Self {
        use std::collections::{BTreeMap, BTreeSet};

        let node_set = nodes.iter().copied().collect::<BTreeSet<_>>();
        let mut dependencies = nodes
            .iter()
            .copied()
            .map(|node| (node, BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();
        for edge in plan.data_edges.iter().chain(&plan.effect_edges) {
            if edge.consumer != edge.dependency
                && node_set.contains(&edge.consumer)
                && node_set.contains(&edge.dependency)
            {
                dependencies
                    .get_mut(&edge.consumer)
                    .expect("induced nodes initialize dependencies")
                    .insert(edge.dependency);
            }
        }
        Self {
            nodes: nodes.to_vec(),
            dependencies: dependencies
                .into_iter()
                .map(|(node, dependencies)| (node, dependencies.into_iter().collect()))
                .collect(),
        }
    }
}

impl ScheduleDag for InducedDag {
    type Node = u64;

    fn nodes(&self) -> Vec<Self::Node> {
        self.nodes.clone()
    }

    fn dependencies(&self, node: Self::Node) -> Vec<Self::Node> {
        self.dependencies.get(&node).cloned().unwrap_or_default()
    }
}

impl ScheduleDag for EpochDag<'_> {
    type Node = u64;

    fn nodes(&self) -> Vec<Self::Node> {
        self.nodes.clone()
    }

    fn dependencies(&self, node: Self::Node) -> Vec<Self::Node> {
        self.dependencies.get(&node).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
