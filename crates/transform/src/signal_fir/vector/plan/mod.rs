//! Construction of the strategy-independent vector plan (loop placement,
//! transports, epochs, fused serial groups).
//!
//! # C++ provenance and adaptation
//! Placement uses `DAGInstructionsCompiler::needSeparateLoop` from
//! `compiler/generator/compile_vect.cpp` and
//! `compiler/generator/dag_instructions_compiler.cpp`. Unlike the C++ pass,
//! this builder never rediscovers occurrence, delay, clock, recursion, type,
//! or effect facts while lowering. It accepts only an independently checked
//! [`VerifiedDecorationCertificate`](crate::signal_fir::decoration_verify::VerifiedDecorationCertificate), allocates stable loop/transport ids, and
//! then calls the independent [`verify_vector_plan`](crate::signal_fir::vector::verify::verify_vector_plan) trust boundary.
//!
//! The result deliberately contains no scheduling order and this API has no
//! `SchedulingStrategy` parameter. `-ss` is applied later, independently in
//! each fixed epoch. Delayed inter-loop uses contribute ordering edges but no
//! immediate-value transports: this is the Rust counterpart of the C++ delay
//! line loop preceding its readers within a vector chunk. The routing stage
//! still owns region-aware FIR routing; the state/clock stages own complete
//! clock-domain epochs and
//! delay/recursion storage geometry.
//!
//! Fused serial groups are an adapted representation of the C++ mutable
//! `CodeLoop` nesting used for state-mediated sample dependencies. Production
//! construction closes sample-required occurrence/data ancestors, every
//! dangerous delayed-read/carrier relation, its same-sample path, and all
//! conflicting effect users. Symbolic recursion carriers and table containers
//! remain structural: their executable children, rather than the containers,
//! enter the sample closure. The independent checker reconstructs these sets
//! before routing can consume the certificate.
//!
//! Development history: P4.4 of
//! `porting/vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`.

pub mod build;
pub(crate) mod fusion;
pub(crate) mod producer_reachability;

pub use build::*;

#[cfg(test)]
use super::verify::verify_vector_plan;
use super::verify::{VectorPlan, VectorPlanError};
use std::fmt;

const EFFECT_ISLAND_TAG: u64 = 1 << 63;
/// Opaque evidence that plan construction was independently verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedVectorPlan {
    plan: VectorPlan,
}
impl VerifiedVectorPlan {
    /// Returns the accepted strategy-independent plan.
    #[must_use]
    pub fn plan(&self) -> &VectorPlan {
        &self.plan
    }

    /// Consumes the evidence wrapper and returns the accepted plan.
    #[must_use]
    pub fn into_plan(self) -> VectorPlan {
        self.plan
    }
}
#[cfg(test)]
pub(crate) fn verified_vector_plan_for_test(plan: VectorPlan) -> VerifiedVectorPlan {
    verify_vector_plan(&plan).expect("test vector plan must satisfy the production verifier");
    VerifiedVectorPlan { plan }
}
/// Why production vector-plan construction failed closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VectorPlanBuildError {
    /// Chunk size must be positive.
    VecSizeZero,
    /// A certified dependency unexpectedly names no certified record.
    MissingRecord {
        /// The dependency endpoint without a certified record.
        signal_id: u32,
    },
    /// A certificate dependency endpoint carries a record and possibly a
    /// placement, but the placement traversal never reached it, so it has no
    /// execution context. Distinct from [`Self::MissingRecord`]: the record
    /// exists, the plan's context model is incomplete.
    MissingContext {
        /// The signal with a record but no execution context.
        signal_id: u32,
    },
    /// A compute-visible sample signal was not reached by occurrence facts.
    SampleSignalUnplaced {
        /// The unplaced sample signal.
        signal_id: u32,
    },
    /// A possible zero-delay state read crosses loops without one serial
    /// execution envelope.
    UnfusedImmediateDelayCrossing {
        /// The loop writing the state.
        producer: u64,
        /// The loop reading the state at a possibly zero delay.
        consumer: u64,
    },
    /// The independent plan verifier rejected the constructed DTO.
    Verification(VectorPlanError),
}
impl fmt::Display for VectorPlanBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VecSizeZero => write!(f, "vector-plan chunk size must be positive"),
            Self::MissingRecord { signal_id } => {
                write!(f, "vector-plan dependency names missing signal {signal_id}")
            }
            Self::MissingContext { signal_id } => {
                write!(
                    f,
                    "vector-plan placement traversal never reached signal {signal_id}: it has a record but no execution context"
                )
            }
            Self::SampleSignalUnplaced { signal_id } => {
                write!(f, "sample signal {signal_id} has no vector placement")
            }
            Self::UnfusedImmediateDelayCrossing { producer, consumer } => write!(
                f,
                "state-mediated immediate delay crosses loop {producer} -> {consumer} without a fused serial group"
            ),
            Self::Verification(error) => write!(f, "constructed vector plan is invalid: {error}"),
        }
    }
}
impl std::error::Error for VectorPlanBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Verification(error) => Some(error),
            _ => None,
        }
    }
}
impl From<VectorPlanError> for VectorPlanBuildError {
    fn from(value: VectorPlanError) -> Self {
        Self::Verification(value)
    }
}

#[cfg(test)]
mod tests;
