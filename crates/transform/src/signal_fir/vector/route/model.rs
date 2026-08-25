//! Routed-FIR vocabulary: region layout, resolutions, transports, the
//! verified trace wrapper, and the error taxonomy.

use crate::schedule::SchedulingStrategy;
use crate::signal_fir::vector::clock_ad::ClockTransportMode;
use crate::signal_fir::vector::schedule::VectorScheduleError;
use crate::signal_fir::vector::verify::{LoopKind, Placement, VectorPlan, VectorPlanError};
use fir::{FirId, FirType};
use std::fmt;
/// Lexical visibility scope used by the routing value cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VectorRegion {
    /// Fixed control/lifecycle scope, visible from every loop.
    Control,
    /// One vector-plan loop region.
    Loop(u64),
}
/// One materialized loop region in strategy-dependent execution order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorLoopRegion {
    /// Stable loop id from the vector plan.
    pub loop_id: u64,
    /// Epoch containing the loop.
    pub epoch_id: u64,
    /// Dependency rank of the epoch.
    pub epoch_rank: u64,
    /// Vectorizable, recursive, or scalar-island loop classification.
    pub kind: LoopKind,
    /// Canonically ordered signal roots owned by the loop.
    pub roots: Vec<u64>,
}
/// Scheduled region layout. The plan remains strategy independent; only this
/// ordered projection varies with `-ss`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorRegionLayout {
    pub(super) strategy: SchedulingStrategy,
    pub(super) loops: Vec<VectorLoopRegion>,
}
impl VectorRegionLayout {
    /// Returns the scheduling strategy that ordered the loop regions.
    #[must_use]
    pub fn strategy(&self) -> SchedulingStrategy {
        self.strategy
    }

    /// Returns the loop regions in scheduled execution order.
    #[must_use]
    pub fn loops(&self) -> &[VectorLoopRegion] {
        &self.loops
    }
}
/// Result of one region-aware cache lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteResolution {
    /// A visible FIR value, direct or loaded from a planned transport.
    Value(FirId),
    /// An `Inline` signal has no value in this exact loop yet and must be
    /// lowered locally before being recorded with
    /// [`VectorRouteSession::define_in_loop`](super::session::VectorRouteSession::define_in_loop).
    NeedsInlineLowering,
}
/// Source selected for one recorded signal use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoutedUseSource {
    /// Value read directly from a visible region.
    Direct(VectorRegion),
    /// Value loaded from the planned transport with this id.
    Transport(u64),
}
/// One FIR definition recorded at its legal visibility scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoutedDefinition {
    /// Stable signal id.
    pub signal_id: u64,
    /// Region in which the FIR value is defined.
    pub region: VectorRegion,
    /// FIR value implementing the signal in that region.
    pub value: FirId,
}
/// One cache resolution made from a loop region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoutedUse {
    /// Stable signal id.
    pub signal_id: u64,
    /// Loop reading the signal.
    pub consumer_loop: u64,
    /// Visibility path selected for the read.
    pub source: RoutedUseSource,
    /// Direct value or concrete transport load.
    pub value: FirId,
}
/// Concrete FIR evidence for one planned transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedTransport {
    /// Stable transport id from the vector plan.
    pub transport_id: u64,
    /// Concrete lifetime/indexing policy selected before FIR construction.
    pub mode: ClockTransportMode,
    /// Stack-array declaration with the canonical transport name and type.
    pub declaration: FirId,
    /// Value defined by the producer loop.
    pub producer_value: Option<FirId>,
    /// Producer-side table store.
    pub store: Option<FirId>,
    /// Consumer-side table load.
    pub load: Option<FirId>,
}
/// Canonical routed-FIR trace checked after lowering and before emission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutedFirTrace {
    pub(super) definitions: Vec<RoutedDefinition>,
    pub(super) uses: Vec<RoutedUse>,
    pub(super) transports: Vec<RoutedTransport>,
}
impl RoutedFirTrace {
    /// Returns every recorded definition.
    #[must_use]
    pub fn definitions(&self) -> &[RoutedDefinition] {
        &self.definitions
    }

    /// Returns every recorded use resolution.
    #[must_use]
    pub fn uses(&self) -> &[RoutedUse] {
        &self.uses
    }

    /// Returns the FIR evidence for every planned transport.
    #[must_use]
    pub fn transports(&self) -> &[RoutedTransport] {
        &self.transports
    }
}
/// Opaque evidence that [`verify_routed_fir`](super::check::verify_routed_fir) accepted the routing trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedRoutedFir {
    pub(super) plan: VectorPlan,
    pub(super) layout: VectorRegionLayout,
    pub(super) trace: RoutedFirTrace,
}
impl VerifiedRoutedFir {
    /// Returns the exact verified plan whose stable identities own this route.
    #[must_use]
    pub fn plan(&self) -> &VectorPlan {
        &self.plan
    }

    /// Returns the scheduled region layout.
    #[must_use]
    pub fn layout(&self) -> &VectorRegionLayout {
        &self.layout
    }

    /// Returns the accepted routed-FIR trace.
    #[must_use]
    pub fn trace(&self) -> &RoutedFirTrace {
        &self.trace
    }

    /// Consumes the evidence and returns the accepted trace.
    #[must_use]
    pub fn into_trace(self) -> RoutedFirTrace {
        self.trace
    }
}
/// Typed routing or routed-FIR verification failure.
#[derive(Clone, Debug, PartialEq)]
pub enum VectorRouteError {
    /// Underlying vector plan verification failed.
    Plan(VectorPlanError),
    /// Vector scheduling failed.
    Schedule(VectorScheduleError),
    /// A signal id is not part of the vector plan.
    UnknownSignal {
        /// Stable signal id.
        signal_id: u64,
    },
    /// A loop id is not part of the vector plan.
    UnknownLoop {
        /// Stable loop id.
        loop_id: u64,
    },
    /// A definition was recorded in a region that contradicts its placement.
    WrongRegion {
        /// Stable signal id.
        signal_id: u64,
        /// Placement required by the plan.
        expected: Placement,
        /// Region the definition was recorded in.
        actual: VectorRegion,
    },
    /// A signal was defined twice in the same region.
    DuplicateDefinition {
        /// Stable signal id.
        signal_id: u64,
        /// Region holding the duplicate definition.
        region: VectorRegion,
    },
    /// A signal has no definition in its required region.
    MissingDefinition {
        /// Stable signal id.
        signal_id: u64,
        /// Region missing the definition.
        region: VectorRegion,
    },
    /// An inline signal was resolved but never lowered.
    MissingInlineDefinition {
        /// Stable signal id.
        signal_id: u64,
    },
    /// No planned transport exists for a cross-loop read.
    MissingTransport {
        /// Stable signal id.
        signal_id: u64,
        /// Loop owning the value.
        producer_loop: u64,
        /// Loop reading the value.
        consumer_loop: u64,
    },
    /// A tuple signal must be routed through its scalar projections.
    UnsupportedTupleTransport {
        /// Stable signal id.
        signal_id: u64,
    },
    /// A tuple value is not a canonical typed component array.
    TupleValueShape {
        /// Stable signal id.
        signal_id: u64,
    },
    /// A recorded FIR value does not have the planned type.
    ValueTypeMismatch {
        /// Stable signal id.
        signal_id: u64,
        /// FIR type required by the plan.
        expected: FirType,
        /// FIR type of the recorded value, if any.
        actual: Option<FirType>,
    },
    /// Routed definitions do not cover a planned signal exactly.
    DefinitionCoverage {
        /// Stable signal id.
        signal_id: u64,
    },
    /// The routed trace does not exactly cover a planned transport.
    TransportCoverage {
        /// Stable transport id.
        transport_id: u64,
    },
    /// A transport declaration has an invalid FIR shape.
    TransportDeclaration {
        /// Stable transport id.
        transport_id: u64,
    },
    /// A transport producer store is invalid or missing.
    TransportStore {
        /// Stable transport id.
        transport_id: u64,
    },
    /// A transport consumer load is invalid or missing.
    TransportLoad {
        /// Stable transport id.
        transport_id: u64,
    },
    /// A recorded use is not visible from its consumer loop.
    InvalidUse {
        /// Stable signal id.
        signal_id: u64,
        /// Loop that recorded the use.
        consumer_loop: u64,
    },
    /// The clock/AD plan does not belong to the routed vector plan.
    ClockPlanMismatch,
    /// Clock transport policies do not exactly cover a planned transport.
    TransportPolicyCoverage {
        /// Stable transport id.
        transport_id: u64,
    },
}
impl fmt::Display for VectorRouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan(error) => write!(f, "vector plan verification failed: {error}"),
            Self::Schedule(error) => write!(f, "vector scheduling failed: {error}"),
            Self::UnknownSignal { signal_id } => write!(f, "unknown signal {signal_id}"),
            Self::UnknownLoop { loop_id } => write!(f, "unknown loop {loop_id}"),
            Self::WrongRegion {
                signal_id,
                expected,
                actual,
            } => write!(
                f,
                "signal {signal_id} has placement {expected:?}, not region {actual:?}"
            ),
            Self::DuplicateDefinition { signal_id, region } => {
                write!(f, "signal {signal_id} is defined twice in {region:?}")
            }
            Self::MissingDefinition { signal_id, region } => {
                write!(f, "signal {signal_id} has no definition in {region:?}")
            }
            Self::MissingInlineDefinition { signal_id } => {
                write!(f, "inline signal {signal_id} was never lowered")
            }
            Self::MissingTransport {
                signal_id,
                producer_loop,
                consumer_loop,
            } => write!(
                f,
                "no planned transport for signal {signal_id} from loop {producer_loop} to loop {consumer_loop}"
            ),
            Self::UnsupportedTupleTransport { signal_id } => write!(
                f,
                "tuple signal {signal_id} must be routed through its scalar projections"
            ),
            Self::TupleValueShape { signal_id } => write!(
                f,
                "tuple value for signal {signal_id} is not a canonical typed component array"
            ),
            Self::ValueTypeMismatch {
                signal_id,
                expected,
                actual,
            } => write!(
                f,
                "signal {signal_id} FIR type {actual:?} does not match {expected:?}"
            ),
            Self::DefinitionCoverage { signal_id } => {
                write!(f, "routed definitions do not cover signal {signal_id}")
            }
            Self::TransportCoverage { transport_id } => {
                write!(
                    f,
                    "routed trace does not exactly cover transport {transport_id}"
                )
            }
            Self::TransportDeclaration { transport_id } => {
                write!(f, "transport {transport_id} declaration is invalid")
            }
            Self::TransportStore { transport_id } => {
                write!(
                    f,
                    "transport {transport_id} producer store is invalid or missing"
                )
            }
            Self::TransportLoad { transport_id } => {
                write!(
                    f,
                    "transport {transport_id} consumer load is invalid or missing"
                )
            }
            Self::InvalidUse {
                signal_id,
                consumer_loop,
            } => write!(
                f,
                "signal {signal_id} is not visible from loop {consumer_loop} through the recorded source"
            ),
            Self::ClockPlanMismatch => {
                write!(f, "clock/AD plan does not belong to the routed vector plan")
            }
            Self::TransportPolicyCoverage { transport_id } => write!(
                f,
                "clock transport policy does not exactly cover transport {transport_id}"
            ),
        }
    }
}
impl std::error::Error for VectorRouteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Plan(error) => Some(error),
            Self::Schedule(error) => Some(error),
            _ => None,
        }
    }
}
impl From<VectorPlanError> for VectorRouteError {
    fn from(value: VectorPlanError) -> Self {
        Self::Plan(value)
    }
}
impl From<VectorScheduleError> for VectorRouteError {
    fn from(value: VectorScheduleError) -> Self {
        Self::Schedule(value)
    }
}
