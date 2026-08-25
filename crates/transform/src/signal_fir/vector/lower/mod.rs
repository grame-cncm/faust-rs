//! Signal-closure lowering into verified vector regions (per-region CSE,
//! bodies checked against routing evidence).
//!
//! C++ `DAGInstructionsCompiler::compileMultiSignal` recursively lowers one
//! loop root and its inline closure while its current loop owns cache lookup.
//! This adapted Rust slice consumes the already verified prepared forest and
//! vector plan, plus the state and clock policies when requested. It runs
//! CSE independently in each routed region, then checks the final bodies
//! against the routing evidence. Storage and transport geometry are never
//! inferred here: fixed or bounded-variable delays, symbolic recursion, and
//! clock wrappers are lowered only through their accepted state/clock
//! artifacts, and table/soundfile reads through their admitted decorations.
//! UI programs, foreign calls, and reverse AD remain fail-closed.
//!
//! Development history: P5.2/P6.5 and the E-phase table/soundfile admissions
//! (see the 2026-07 journal).

pub mod check;
pub mod program;
pub mod signal;
pub mod tables;

pub use check::*;
pub use program::*;
pub use signal::*;
pub(in crate::signal_fir::vector) use tables::mutable_table_name;

#[cfg(test)]
mod tests;
