//! Region-aware vector routing and routed-FIR verification (three
//! visibility scopes, planned transports only).
//!
//! # C++ provenance and adaptation
//! C++ `DAGInstructionsCompiler` combines loop ownership, value caching, and
//! vector buffer allocation while recursively compiling signals. Rust keeps
//! those concerns explicit: the vector plan freezes a [`VectorPlan`](crate::signal_fir::vector::verify::VectorPlan), this module resolves
//! values through three visibility scopes, and later lowering emits the loop
//! bodies. A cross-loop read can only use a transport already named by the plan;
//! no buffer identity is allocated on demand.
//!
//! C++ compiles a recursive tuple through its individual `sigProj` values; it
//! does not allocate an inter-loop array of tuple objects. Rust retains the
//! tuple as a canonical typed `ValueArray` in routing evidence so simultaneous
//! recursion can be checked, but rejects tuple transports: only the scalar
//! projections may cross loop boundaries. The checker recursively validates
//! tuple arity and component types instead of trusting the outer FIR type.
//!
//! Routing emits real FIR declarations, stores, and loads for planned
//! transports and independently verifies them; the production final-module
//! path consumes the verified result. When a checked clock/AD plan is
//! supplied, declarations and accesses use its exact outer-chunk,
//! island-scalar, or held-output lifetime; the assembly stage places those
//! words in the corresponding final region bodies.
//!
//! Development history: P5.1/P6.2/P6.3b slices of
//! `porting/vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`.

pub mod check;
pub mod model;
pub mod session;

pub use check::*;
pub use model::*;
pub use session::*;

#[cfg(test)]
mod tests;
