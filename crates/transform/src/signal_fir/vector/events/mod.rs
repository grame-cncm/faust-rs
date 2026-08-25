//! Bounded event-order certificates for vector loop fission
//! (`FissionSafe`: scalar-ordered dynamic dependences stay ordered).
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
//! The model is deliberately bounded. Its base form is the structural gate on
//! the routed chunk; its state-refined form consumes the state plan's
//! `DelaySim`/`RecStep` evidence and
//! replaces the corresponding conservative effects with explicit
//! `LoopPre`/sample/`LoopPost` events. Neither form proves complete DSP
//! semantics. Production construction and independent checking require an
//! explicit event limit and fail closed when neither the complete chunk nor the
//! independently reconstructed two-sample basis fits it.

pub mod check;
pub mod model;
pub mod produce;

pub use check::*;
pub use model::*;
pub use produce::*;

#[cfg(test)]
mod tests;
