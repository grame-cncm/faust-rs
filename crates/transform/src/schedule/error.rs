//! Typed errors for [`crate::schedule::schedule`] and
//! [`crate::schedule::verify_schedule`].

use std::fmt;

/// Why [`crate::schedule::schedule`] could not produce an order.
///
/// Both variants are detected once, strategy-independently, before any of
/// the four literal algorithms runs (`validate::ensure_schedulable`): a
/// malformed graph never reaches the `DepthFirst`/`BreadthFirst`/`Special`/
/// `ReverseBreadthFirst` walk, so none of them can hang or return a partial
/// order. The C++ originals (`compiler/DirectedGraph/Schedule.hh`) assume a
/// DAG and perform no such check — an instantaneous or longer cycle in C++
/// input would recurse forever (`dfschedule`/`recschedulenode`) or loop
/// forever (`parallelize`'s memoized `level`), which is exactly the failure
/// mode this validation exists to rule out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScheduleError<N> {
    /// A node depends on itself, directly. Reported before the general
    /// cycle check: an instantaneous self-dependency is never a legal
    /// same-loop edge (plan §4.1) — an adapter that intends a same-node loop
    /// edge must normalize it away before presenting the DAG here.
    SelfEdge {
        /// The offending node (the first one found in `nodes()` order).
        node: N,
    },
    /// A dependency cycle of length ≥ 2. `remaining` is every node that
    /// Kahn peeling could not remove: the cycle's own members plus every
    /// node that transitively depends on one, stable-sorted ascending.
    Cycle {
        /// The unschedulable nodes, ascending.
        remaining: Vec<N>,
    },
}

impl<N: fmt::Debug> fmt::Display for ScheduleError<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfEdge { node } => write!(f, "node {node:?} depends on itself"),
            Self::Cycle { remaining } => write!(
                f,
                "dependency cycle among {} node(s): {remaining:?}",
                remaining.len()
            ),
        }
    }
}

impl<N: fmt::Debug> std::error::Error for ScheduleError<N> {}

/// Why [`crate::schedule::verify_schedule`] rejected a candidate order.
///
/// This is the small independent checker (plan §5.10 `verify_schedule`,
/// mirrored by the Lean `verifySchedule`/`coversB`/`respectsDependenciesB` in
/// `porting/vector-mode-scheduling-formal-spec.lean`): it never re-runs a
/// scheduling algorithm, only checks the two S-Sound/S-Complete properties
/// directly against the candidate order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyError<N> {
    /// `node` occurs more than once in `dag.nodes()` itself: the *graph* is
    /// malformed, so no order can legitimately cover it. Mirrors the Lean
    /// `coversB`'s `noDuplicatesB graph.nodes` clause (see the comment above
    /// `coversB` in `porting/vector-mode-scheduling-formal-spec.lean`):
    /// without this check, a graph whose node list contained duplicates
    /// would be "covered" by its deduplicated order, making the checker
    /// strictly weaker than the mathematical coverage predicate. Checked
    /// before every order-side check so the reported error is deterministic
    /// regardless of the candidate order.
    DuplicateGraphNode {
        /// The node repeated in `dag.nodes()` (first repeat encountered in
        /// `nodes()` order).
        node: N,
    },
    /// `node` occurs more than once in the candidate order.
    Duplicate {
        /// The repeated node.
        node: N,
    },
    /// `node` is a DAG node absent from the candidate order.
    Missing {
        /// The omitted node.
        node: N,
    },
    /// `node` occurs in the candidate order but is not a DAG node.
    Extra {
        /// The unexpected node.
        node: N,
    },
    /// `dependency` does not occur strictly before `consumer` in the
    /// candidate order (either it is scheduled at or after `consumer`, or it
    /// is missing from the order entirely).
    OutOfOrder {
        /// The consumer whose dependency was not scheduled first.
        consumer: N,
        /// The dependency that should have preceded `consumer`.
        dependency: N,
    },
}

impl<N: fmt::Debug> fmt::Display for VerifyError<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateGraphNode { node } => write!(
                f,
                "graph node list contains {node:?} more than once (malformed adapter)"
            ),
            Self::Duplicate { node } => write!(f, "node {node:?} appears more than once"),
            Self::Missing { node } => write!(f, "node {node:?} is missing from the order"),
            Self::Extra { node } => write!(f, "node {node:?} is not a node of the graph"),
            Self::OutOfOrder {
                consumer,
                dependency,
            } => write!(
                f,
                "dependency {dependency:?} does not precede consumer {consumer:?}"
            ),
        }
    }
}

impl<N: fmt::Debug> std::error::Error for VerifyError<N> {}
