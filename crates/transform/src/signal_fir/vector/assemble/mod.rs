//! Verified FIR assembly of vector loops, state phases, and clock islands
//! (independent exact-coverage inspection of the emitted FIR).
//!
//! # C++ provenance and adaptation
//! State words follow `DAGInstructionsCompiler::generateDlineLoop` and
//! `generateDelayAccess` in `compiler/generator/dag_instructions_compiler.cpp`:
//! short delays copy `_perm` history into `_tmp`, execute the chunk, then copy
//! the tail back; long delays advance a masked ring index before the chunk and
//! save the chunk count afterwards. Recursive projections are captured into
//! stack temporaries before any projection storage is updated, preserving the
//! simultaneous `sigRec`/`sigProj` step.
//!
//! Clock guards follow the scalar `SignalFIRLowerer` implementation of OD, US,
//! and DS. Unlike the C++ compiler's mutable `CodeLoop` tree, Rust assembles an
//! immutable, checked artifact from the accepted plan/route/state/clock
//! artifacts, keeps recursion-step declarations in the enclosing sample
//! scope, and
//! places held clock outputs after the island guard, so a non-firing sample
//! observes the previous held value. Final lifecycle placement is checked by
//! `vector_module`. Clocked composition adds one checked shared state cursor
//! per clock domain;
//! it advances at the end of each guarded fire. Section 8 lockstep bundles are
//! adapted as one physical `i0` loop whose body contains the unchanged scalar
//! iteration of every lane in canonical order. This preserves each lane's IEEE
//! operation and contraction policy while exposing cross-instance SLP to FIR
//! backends without changing the planar `compute` ABI.
//!
//! General fused serial groups use the same physical sample envelope. A
//! top-rate group owns one `i0` body; a nonzero-clock group owns one contiguous
//! sequence inside exactly one guarded clock island. The independent assembly
//! checker requires delayed reads, state writes, and scalarized internal
//! transports to remain inside that envelope.

pub mod check;
pub mod materialize;
pub mod model;

pub use check::*;
pub use materialize::*;
pub use model::*;

#[cfg(test)]
mod tests;
