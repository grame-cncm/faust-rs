//! Pure-vector program artifacts: region bodies, the verified program
//! wrapper, the lowering context, and the error taxonomy.

use crate::schedule::SchedulingStrategy;
use crate::signal_fir::ControlRateMode;
use crate::signal_fir::FirOrigins;
use crate::signal_fir::vector::analysis::EffectAtom;
use crate::signal_fir::vector::route::{VectorRegion, VectorRouteError, VerifiedRoutedFir};
use crate::signal_fir::vector::verify::ValueType;
use crate::signal_prepare::SimpleSigType;
use fir::{FirId, FirMathOp, FirStore, FirType};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
/// One scheduled vector loop and its final CSE-rewritten FIR body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PureVectorRegionBody {
    pub(super) loop_id: u64,
    pub(super) statements: Vec<FirId>,
}
impl PureVectorRegionBody {
    /// Stable planned-loop identity.
    #[must_use]
    pub fn loop_id(&self) -> u64 {
        self.loop_id
    }

    /// Final statements in execution order.
    #[must_use]
    pub fn statements(&self) -> &[FirId] {
        &self.statements
    }
}
/// Opaque lowering result accepted by routing and region-body verification.
///
/// The historical `Pure` name is retained for source compatibility. The
/// representation now also carries programs accepted through explicit state
/// and clock policies; it does not imply that those programs are
/// pure.
pub struct VerifiedPureVectorProgram {
    pub(super) store: FirStore,
    pub(super) origins: FirOrigins,
    pub(super) static_declarations: Vec<FirId>,
    pub(super) table_declarations: Vec<FirId>,
    pub(super) table_init_statements: Vec<FirId>,
    /// Fill statements for file-scope generated tables; the body of
    /// `staticInit` (rendered as `classInit`). Empty unless `--table-init
    /// runtime` produced a read-only generated table.
    pub(super) static_init_statements: Vec<FirId>,
    /// `SubModule` nodes for generated tables, in allocation order.
    pub(super) sub_modules: Vec<FirId>,
    pub(super) mutable_tables: BTreeMap<u64, (String, usize, FirType)>,
    pub(super) transport_declarations: Vec<FirId>,
    pub(super) control_statements: Vec<FirId>,
    /// Externalizable control-rate statements (UI snapshots, promoted
    /// control-root stores, control-scope UI effect stores). Empty in the
    /// classic inline mode; the body of `control(dsp)` under `-ec`.
    pub(super) external_control_statements: Vec<FirId>,
    /// DSP struct fields created by `-ec` promotion (snapshots + promoted
    /// control temporaries).
    pub(super) control_state_fields: Vec<(String, FirType)>,
    pub(super) regions: Vec<PureVectorRegionBody>,
    pub(super) routed: VerifiedRoutedFir,
    pub(super) math_ops: HashSet<FirMathOp>,
    pub(super) int_helpers: BTreeSet<&'static str>,
}
impl VerifiedPureVectorProgram {
    /// FIR store owning every returned id.
    #[must_use]
    pub fn store(&self) -> &FirStore {
        &self.store
    }

    /// Mutable store access reserved for the checked final-module assembler.
    pub(crate) fn store_mut(&mut self) -> &mut FirStore {
        &mut self.store
    }

    /// Consumes the checked program and returns its FIR store plus provenance.
    pub(crate) fn into_store_and_origins(self) -> (FirStore, FirOrigins) {
        (self.store, self.origins)
    }

    /// Propagates direct Signal producers through the assembled module.
    pub(crate) fn derive_origins(&mut self, module: FirId) {
        self.origins.derive_reachable(&self.store, module);
    }

    /// Canonical transport declarations emitted before region bodies.
    #[must_use]
    pub fn transport_declarations(&self) -> &[FirId] {
        &self.transport_declarations
    }

    /// Immutable literal tables required by checked waveform reads.
    #[must_use]
    pub fn static_declarations(&self) -> &[FirId] {
        &self.static_declarations
    }

    /// Mutable table DSP-struct field declarations.
    #[must_use]
    pub fn table_declarations(&self) -> &[FirId] {
        &self.table_declarations
    }

    /// Element-wise mutable-table initialization for `instanceConstants`.
    #[must_use]
    pub fn table_init_statements(&self) -> &[FirId] {
        &self.table_init_statements
    }

    /// Fill statements for file-scope generated tables (`staticInit`).
    #[must_use]
    pub fn static_init_statements(&self) -> &[FirId] {
        &self.static_init_statements
    }

    /// Generated-table sub-modules carried by this program.
    #[must_use]
    pub fn sub_modules(&self) -> &[FirId] {
        &self.sub_modules
    }

    /// Accepted mutable tables by signal id: field name, length, element type.
    #[must_use]
    pub fn mutable_tables(&self) -> &BTreeMap<u64, (String, usize, FirType)> {
        &self.mutable_tables
    }

    /// Fixed control-scope statements, including input pointer aliases.
    #[must_use]
    pub fn control_statements(&self) -> &[FirId] {
        &self.control_statements
    }

    /// Externalizable control statements (the `control` body under `-ec`).
    #[must_use]
    pub fn external_control_statements(&self) -> &[FirId] {
        &self.external_control_statements
    }

    /// Struct fields created by external-control promotion.
    #[must_use]
    pub fn control_state_fields(&self) -> &[(String, FirType)] {
        &self.control_state_fields
    }

    /// Loop bodies in the selected strategy-dependent schedule order.
    #[must_use]
    pub fn regions(&self) -> &[PureVectorRegionBody] {
        &self.regions
    }

    /// Independently accepted route evidence.
    #[must_use]
    pub fn routed(&self) -> &VerifiedRoutedFir {
        &self.routed
    }

    /// Math prototypes required when this artifact is assembled as a module.
    #[must_use]
    pub fn math_ops(&self) -> &HashSet<FirMathOp> {
        &self.math_ops
    }

    /// Integer helper prototypes required by `min`, `max`, or `abs`.
    #[must_use]
    pub fn int_helpers(&self) -> &BTreeSet<&'static str> {
        &self.int_helpers
    }
}
/// Pure-lowering or final-body verification failure.
#[derive(Clone, Debug, PartialEq)]
pub enum PureVectorLowerError {
    /// Route construction or verification failed.
    Route(VectorRouteError),
    /// Internal real precision is outside the active fast-lane contract.
    InvalidRealType(FirType),
    /// A planned signal id is absent from the verified prepared forest.
    MissingPreparedSignal {
        /// The planned signal id with no prepared counterpart.
        signal_id: u64,
    },
    /// Prepared and planned scalar types disagree.
    PlannedTypeMismatch {
        /// The signal whose types disagree.
        signal_id: u64,
        /// Scalar type recorded by the vector plan.
        planned: ValueType,
        /// Scalar type recorded by the prepared forest, if any.
        prepared: Option<SimpleSigType>,
    },
    /// The pure lowering slice cannot execute an effect-bearing signal.
    EffectfulSignal {
        /// The effect-bearing signal.
        signal_id: u64,
        /// Rendered signal expression for the diagnostic.
        expression: String,
        /// The effect atoms that make the signal impure.
        effects: Vec<EffectAtom>,
    },
    /// The pure lowering slice has no state/effect semantics for this node.
    UnsupportedSignal {
        /// The unsupported signal.
        signal_id: u64,
        /// Rendered signal expression for the diagnostic.
        expression: String,
    },
    /// A control expression depended on a sample-region value.
    InvalidControlDependency {
        /// The control signal with the sample-region dependency.
        signal_id: u64,
    },
    /// A pure signal cycle escaped the planned recursion boundary.
    PureCycle {
        /// A signal on the escaped cycle.
        signal_id: u64,
        /// The region in which the cycle was found.
        region: VectorRegion,
    },
    /// Audio input index is invalid for the declared module arity.
    InputIndexOutOfRange {
        /// The out-of-range input index.
        index: i32,
        /// The module's declared number of audio inputs.
        num_inputs: usize,
    },
    /// FIR operands or result violate the prepared typing contract.
    FirTypeMismatch {
        /// The signal whose FIR typing is inconsistent.
        signal_id: u64,
        /// The FIR type required by the contract.
        expected: FirType,
        /// The FIR type actually found, if any.
        actual: Option<FirType>,
    },
    /// Region-local CSE did not preserve one sink per requested root.
    CseRootCoverage {
        /// The region whose root coverage is broken.
        region: VectorRegion,
    },
    /// Final bodies do not contain the evidence accepted by routing.
    BodyEvidence {
        /// Which piece of evidence is missing or different.
        detail: String,
    },
}
impl fmt::Display for PureVectorLowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Route(error) => write!(f, "vector routing failed: {error}"),
            Self::InvalidRealType(typ) => write!(f, "unsupported vector real type {typ:?}"),
            Self::MissingPreparedSignal { signal_id } => {
                write!(
                    f,
                    "vector plan signal {signal_id} is absent from the prepared forest"
                )
            }
            Self::PlannedTypeMismatch {
                signal_id,
                planned,
                prepared,
            } => write!(
                f,
                "signal {signal_id} planned type {planned:?} disagrees with prepared type {prepared:?}"
            ),
            Self::EffectfulSignal {
                signal_id,
                expression,
                effects,
            } => {
                write!(
                    f,
                    "signal {signal_id} is effectful and cannot enter pure P5.2 lowering: {expression}; effects={effects:?}"
                )
            }
            Self::UnsupportedSignal {
                signal_id,
                expression,
            } => write!(
                f,
                "signal {signal_id} is outside the pure P5.2 node set: {expression}"
            ),
            Self::InvalidControlDependency { signal_id } => {
                write!(f, "control lowering reached sample signal {signal_id}")
            }
            Self::PureCycle { signal_id, region } => {
                write!(f, "pure signal cycle at signal {signal_id} in {region:?}")
            }
            Self::InputIndexOutOfRange { index, num_inputs } => {
                write!(f, "input index {index} is outside num_inputs={num_inputs}")
            }
            Self::FirTypeMismatch {
                signal_id,
                expected,
                actual,
            } => write!(
                f,
                "signal {signal_id} FIR type {actual:?} does not match {expected:?}"
            ),
            Self::CseRootCoverage { region } => {
                write!(f, "CSE changed root-sink coverage in {region:?}")
            }
            Self::BodyEvidence { detail } => {
                write!(f, "routed region-body verification failed: {detail}")
            }
        }
    }
}
impl std::error::Error for PureVectorLowerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Route(error) => Some(error),
            _ => None,
        }
    }
}
impl From<VectorRouteError> for PureVectorLowerError {
    fn from(value: VectorRouteError) -> Self {
        Self::Route(value)
    }
}
/// Shared immutable configuration for one vector-region lowering pipeline.
///
/// State and clock certificates remain explicit arguments because they are
/// independently verified artifacts. This context groups the execution policy
/// and module-interface parameters consumed throughout lowering.
pub struct VectorLoweringContext<'a> {
    /// Canonical grouped UI program associated with the prepared forest.
    pub ui: &'a ui::UiProgram,
    /// Per-epoch scheduling strategy.
    pub strategy: SchedulingStrategy,
    /// Internal FIR real type.
    pub real_type: FirType,
    /// Number of audio inputs exposed by the module contract.
    pub num_inputs: usize,
    /// Control-rate evaluation scheduling (`-ec`). With `External`, UI zone
    /// reads are snapshotted into promoted DSP fields, control-root
    /// temporaries are struct-promoted, and the whole externalizable control
    /// section moves to a `control` entry point (plan phase 5).
    pub control_rate_mode: ControlRateMode,
    /// Enclosing module name; a generator sub-module is named `{module}SIG{k}`.
    pub module_name: &'a str,
    /// Whether table generators are folded or compiled into sub-modules.
    pub table_init_mode: crate::signal_fir::TableInitMode,
    /// Explicit SR used to fold `ma.SR` in const table generators.
    pub table_init_sample_rate: Option<i32>,
    /// Delay policy inherited by a generator sub-module.
    pub max_copy_delay: u32,
    /// Delay policy inherited by a generator sub-module.
    pub delay_line_threshold: u32,
    /// Signal-level table protection contract (`-ct`). Lowering never
    /// re-clamps; the flag gates the staging debug assertion and is
    /// inherited by generator sub-modules.
    pub check_table: bool,
}
