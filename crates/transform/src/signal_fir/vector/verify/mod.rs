//! Strategy-independent `VectorPlan` DTO and its independent verifier
//! (`verify_vector_plan`).
//!
//! Before emission, `verify_vector_plan` establishes the formal gate: the
//! `L-*` obligations, typed transports, region visibility, and a `VecSafe`
//! witness per vectorizable loop. This is the vector-plan analogue of the
//! schedule certificate (`crate::schedule::certificate`): a canonical DTO
//! mirroring the
//! `vectorPlan` shape of
//! `porting/schemas/vector-verification-certificate-v2.schema.json`, plus a
//! checker that re-derives every invariant from the plan's own fields. Schema
//! v2 adds lockstep bundles and explicit transport layouts; v1 plans are not
//! silently accepted by the v2 verifier.
//!
//! The checks mechanize the Lean `VectorPlanCertificate` obligations
//! (`porting/vector-mode-scheduling-formal-spec.lean`): unique ids, exact
//! epoch coverage with unique ranks, ownership/root agreement, inline
//! duplicability, complete non-self loop edges, an acyclic induced graph per
//! epoch, well-typed transports, monotone cross-epoch barriers, serial
//! recursion/island loops, and a `VecSafe` witness for every vectorizable
//! loop.
//!
//! # Scope, deliberately bounded
//! The plan builder constructs accepted plans from verified decorations and
//! the routing/lowering stages consume them in production. Two obligations
//! remain deferred, matching the certified plan's own staging:
//! - **effect commutation** (`L-Effects` for incomparable loops): the DTO
//!   retains the decoration analysis's exact effect identities and the verifier derives
//!   duplicability and local `VecSafe` instead of trusting producer booleans,
//!   but it does not yet prove pairwise commutation of independent effectful
//!   loops (the plan calls this the hard case; effect edges are
//!   producer-supplied here);
//! - **JSON (de)serialization / `plan_hash`** (the deferred canonical-JSON
//!   boundary): a
//!   plan is identified by its Rust type, not a runtime tag or hash.

pub mod check;
pub mod checker_reachability;
pub mod error;
pub mod fused_groups;
pub mod model;

pub use super::analysis::EffectAtom;
pub use check::*;
pub(crate) use check::{effects_duplicable, effects_sample_reorderable};
pub use error::*;
pub use fused_groups::*;
pub use model::*;

#[cfg(test)]
mod tests;
