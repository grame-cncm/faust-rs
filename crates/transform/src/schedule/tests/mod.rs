//! Unit tests for the generic scheduler core (P1).
//!
//! - `fixtures`: the minimal [`super::ScheduleDag`] test double (`VecDag`)
//!   and the hand-built graphs every other file schedules.
//! - `exact_orders`: item 1/2 — literal snapshots derived by hand from the
//!   plan §5.4 pseudocode for each strategy on each fixture, each also
//!   passed through `verify_schedule`.
//! - `verify_tests`: item 3 — `verify_schedule` rejection scenarios.
//! - `cycles`: item 6 — self-edge and cycle diagnostics, uniform across all
//!   four strategies.
//! - `exhaustive`: item 4 — every upper-triangular DAG on up to six nodes.
//! - `relabel`: item 5 — order-preserving node relabelling, scrambling
//!   builder insertion order, must commute with scheduling.
//! - `growth`: item 8 — the `Special` growth guardrail on a path-heavy
//!   ladder.

mod cycles;
mod exact_orders;
mod exhaustive;
mod fixtures;
mod growth;
mod relabel;
mod verify_tests;
