//! Generic dependency-DAG scheduler core (vectorization port plan phase P1).
//!
//! # Source provenance (C++)
//! - `compiler/DirectedGraph/Schedule.hh` — `schedule<N>`, `dfschedule`,
//!   `bfschedule`, `spschedule`, `rbschedule`.
//! - `compiler/DirectedGraph/DirectedGraphAlgorythm.hh` — `roots`,
//!   `parallelize`, `reverse`, `interleave`, `recschedulenode`,
//!   `recschedule`.
//! - Plan:
//!   `porting/vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`
//!   §2.5 (`-ss` behavior), §4.1 (generic scheduling contract), §5.4 (formal
//!   scheduling contract), "P1 - Generic scheduler core".
//! - Formal cross-check: `porting/vector-mode-scheduling-formal-spec.lean`
//!   (`SchedulingStrategy`, `decodeStrategy`, `verifySchedule`,
//!   `diamondGraph`).
//!
//! # Edge convention
//! An edge `consumer -> dependency` — i.e. `dag.dependencies(consumer)`
//! contains `dependency` — means **the dependency must be scheduled before
//! the consumer**. This is the C++ `digraph<N>::destinations` convention: a
//! node's "destinations" are what it depends on, not what depends on it. A
//! **root** is a terminal consumer with no incoming edge (nothing depends on
//! it); C++ `roots(G)` computes it the same way this port does — count how
//! often each node appears as *someone else's* dependency, and keep the ones
//! whose count is zero.
//!
//! # Determinism rule
//! Every strategy ties by **ascending node key** (`Ord` on
//! [`ScheduleDag::Node`]), never by hash-map iteration order:
//! `BreadthFirst`/`ReverseBreadthFirst` sort level buckets by
//! `(level, key)`; `DepthFirst`/`Special` visit `roots(G)` — itself
//! ascending by the [`ScheduleDag`] contract — in that order. **C++ tie
//! order is explicitly not a parity target**: C++ signal ties follow `Tree`
//! pointer identity and vector-loop ties follow `Loop*` pointer identity,
//! neither of which is reproducible or meaningful across runs or platforms.
//! What *is* a parity target is level **membership** (which nodes share a
//! computed height) and dependency **validity** (every produced order is a
//! legal topological order) — both checked structurally by
//! [`verify_schedule`], and, for the literal algorithms, cross-checked by
//! hand against the plan's worked examples and the Lean `diamondGraph`
//! fixture (`tests::exact_orders`).
//!
//! # API mapping status
//!
//! | Item | Status | Rationale |
//! |---|---|---|
//! | [`SchedulingStrategy`] | `adapted` | Same four strategies as `-ss`, as a Rust `enum` instead of an integer flag; [`SchedulingStrategy::decode`] matches the C++ `0 / 1 / 2 / n>=3` split exactly. |
//! | [`ScheduleDag`] | `adapted` | C++ has no such trait — `digraph<N>` is used directly by every algorithm. The trait is the Rust seam letting one algorithm implementation serve every graph shape (`hgraph::Digraph`, `signal_fir::LoopGraph`, future callers) instead of copying the four algorithms per graph type. |
//! | [`schedule`] | `adapted` | Literal port of the four algorithms, plus typed, total cycle/self-edge detection that C++ does not perform (C++ assumes a DAG and recurses/loops unconditionally). |
//! | [`verify_schedule`] | `adapted` | No C++ equivalent; mirrors the Lean `verifySchedule` as an independent postcondition checker (plan §5.10) never reusing a scheduling algorithm. |
//!
//! [`certificate`] adds the canonical, hashable `GraphSnapshot` /
//! `ScheduleCertificate` artifact layer around [`schedule`] /
//! [`verify_schedule`] (certified plan "R1 - Schedule certificate at L2").
//!
//! # Status
//! Purely additive: nothing in the production compile path constructs a
//! [`ScheduleDag`] or calls [`schedule`] yet. Phase P2 (done) threads the
//! public `-ss` / `--scheduling-strategy` option through the compiler down
//! to `SignalFirOptions`, but no lowering path invokes this scheduler; phase
//! P3 activates scalar scheduling. `hgraph::schedule` keeps its own literal
//! depth-first walk unchanged; it is not yet expressed in terms of this
//! module.

mod adapters;
mod breadth_first;
pub mod certificate;
mod common;
mod dag;
mod depth_first;
mod error;
mod reverse_breadth_first;
mod special;
mod validate;
mod verify;

#[cfg(test)]
mod tests;

pub use dag::ScheduleDag;
pub use error::{ScheduleError, VerifyError};
pub use verify::verify_schedule;

/// The `-ss` / `--scheduling-strategy` policy (plan §2.5, C++
/// `compiler/DirectedGraph/Schedule.hh`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SchedulingStrategy {
    /// `-ss 0` (default). C++ `dfschedule`.
    DepthFirst,
    /// `-ss 1`. C++ `bfschedule`.
    BreadthFirst,
    /// `-ss 2`. C++ `spschedule`.
    Special,
    /// `-ss n`, `n >= 3`. C++ `rbschedule`.
    ReverseBreadthFirst,
}

impl SchedulingStrategy {
    /// Total decode of the `-ss` integer contract:
    /// `0 -> DepthFirst`, `1 -> BreadthFirst`, `2 -> Special`,
    /// `n >= 3 -> ReverseBreadthFirst`. Matches the Lean `decodeStrategy`.
    #[must_use]
    pub fn decode(n: u32) -> Self {
        match n {
            0 => Self::DepthFirst,
            1 => Self::BreadthFirst,
            2 => Self::Special,
            _ => Self::ReverseBreadthFirst,
        }
    }
}

/// Serializes `dag` under `strategy`: a total topological order where every
/// dependency precedes its consumer (S-Sound), containing every node exactly
/// once (S-Complete), computed deterministically from `(strategy, dag)`
/// (S-Deterministic), and always returning `Ok` or a typed error — never
/// hanging or emitting a partial order (S-Terminating). These are the four
/// scheduler obligations of plan §5.4.
///
/// Cycle/self-edge detection (`validate::ensure_schedulable`) runs once,
/// before dispatching to the selected literal algorithm, so it is uniform
/// across all four strategies; the C++ originals assume a DAG and do not
/// check at all.
///
/// # Errors
/// [`ScheduleError::SelfEdge`] if any node depends on itself;
/// [`ScheduleError::Cycle`] if `dag` is not acyclic.
pub fn schedule<D: ScheduleDag>(
    strategy: SchedulingStrategy,
    dag: &D,
) -> Result<Vec<D::Node>, ScheduleError<D::Node>> {
    let nodes = validate::ensure_schedulable(dag)?;
    let order = match strategy {
        SchedulingStrategy::DepthFirst => depth_first::run(dag, &nodes),
        SchedulingStrategy::BreadthFirst => breadth_first::run(dag, &nodes),
        SchedulingStrategy::Special => special::run(dag, &nodes),
        SchedulingStrategy::ReverseBreadthFirst => reverse_breadth_first::run(dag, &nodes),
    };
    debug_assert!(
        verify::verify_schedule(dag, &order).is_ok(),
        "internal scheduler bug: {strategy:?} produced an order verify_schedule rejects: {order:?}"
    );
    Ok(order)
}
