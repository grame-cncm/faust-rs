//! Box-to-signal propagation (Phase 4, section 2.4).
//!
//! # Source provenance (C++)
//! - `/Users/letz/Developpements/RUST/faust/compiler/propagate/propagate.hh`
//! - `/Users/letz/Developpements/RUST/faust/compiler/propagate/propagate.cpp`
//! - `/Users/letz/Developpements/RUST/faust/compiler/boxes/boxtype.cpp`
//!
//! # Current scope
//! - Core box arity inference for supported box families.
//! - Primitive lowering from `boxes::BoxMatch` to `signals::SigBuilder`.
//! - Composition algebra: `seq`, `par`, `split`, `merge`.
//! - Explicit typed errors for unsupported nodes and arity mismatches.
//! - Recursive composition lowering with De Bruijn-style placeholders (`sigRec/sigProj` shape).
//! - Typed `FlatBoxId` boundary that validates the post-`eval/a2sb` flat box subset.
//!
//! # Public API mapping status
//! - [`box_arity_typed`] and [`propagate_typed`] are the primary Rust entry
//!   points for the post-`eval/a2sb` flat-box contract.
//! - [`box_arity`] and [`propagate`] remain compatibility wrappers for callers
//!   that still hold a raw `BoxId`.
//! - [`PropagateOutput`] and [`propagate_typed_with_ui`] are the grouped-UI
//!   ownership extensions introduced by the UI IR rewrite.
//! - `make_sig_input_list(...)` mirrors C++ `makeSigInputList(...)`.
//! - `FlatBoxId` / [`try_build_flat_box`] are an adapted Rust boundary: they make the
//!   C++ post-`evalprocess -> a2sb -> propagate` flat-box contract explicit while
//!   preserving `TreeArena` node sharing through `TreeId`.
//! - `route`, `ffun`, `soundfile`, `ondemand`, `upsampling`, and
//!   `downsampling` now lower through the typed flat boundary.
//!
//! # Integer convention
//! - Integer signals emitted by this pass are `i32`-semantic.
//! - Conversions from container sizes/indices (`usize`) are explicit and
//!   fallible to preserve deterministic diagnostics on overflow.

use std::fmt::{Display, Formatter};

use ahash::AHashMap;
use boxes::{BoxId, BoxMatch, match_box};
use errors::codes;
use errors::{Diagnostic, IntoDiagnostic, Severity, Stage};
use signals::{SigBuilder, SigId, SigMatch, match_sig};
use tlib::{NodeKind, TreeArena, TreeId, list_to_vec, tree_to_int, vec_to_list};
use ui::{
    ControlId, ControlKind, ControlRange, ControlSpec, UiGroupKind, UiGroupPathSegment,
    UiGroupSpec, UiMatch, UiMetadata, UiProgram, UiProgramBuilder, UiRootOrigin,
    canonicalize_group_spec, match_ui, normalize_group_label_navigation,
    normalize_widget_label_path, split_label_metadata,
};

/// Memoization cache for [`box_arity`] / [`box_arity_typed`] results, keyed by validated flat boxes.
pub type ArityCache = AHashMap<FlatBoxId, Result<BoxArity, PropagateError>>;
/// Environment mapping route/slot placeholders to propagated signals.
type SlotEnv = AHashMap<BoxId, SigId>;
/// Deterministic mapping from source widget/soundfile box nodes to stable control ids.
type ControlIds = AHashMap<BoxId, ControlId>;

pub const CRATE_NAME: &str = "propagate";
const DEBRUIJN_TAG: &str = "DEBRUIJN";
const DEBRUIJNREF_TAG: &str = "DEBRUIJNREF";

/// Stable crate identifier used in workspace-level tooling and diagnostics.
#[must_use]
pub fn crate_id() -> &'static str {
    CRATE_NAME
}

/// Input/output arity of one box expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Input/output arity summary for one box expression.
pub struct BoxArity {
    /// Number of required input signals.
    pub inputs: usize,
    /// Number of produced output signals.
    pub outputs: usize,
}

/// Explicit products of the post-`eval/a2sb` propagation boundary.
///
/// # Source provenance (C++)
/// - `compiler/propagate/propagate.cpp`
/// - `compiler/signals/signals.hh`
/// - `compiler/signals/signals.cpp`
///
/// Mapping status:
/// - `adapted` relative to the C++ clock-environment/path ownership.
/// - Behaviorally equivalent target: DSP signals and grouped UI become
///   explicit products of propagation instead of backend-local heuristics.
#[derive(Debug)]
pub struct PropagateOutput {
    /// Final propagated output signal list (`box_arity.outputs` items).
    pub signals: Vec<SigId>,
    /// Canonical grouped UI artifact extracted from the same propagated box
    /// tree.
    ///
    /// This is the Rust ownership split that replaces the earlier
    /// backend-local UI reconstruction heuristic: signals carry only control
    /// references, while grouped layout and metadata are owned here.
    pub ui: UiProgram,
}

/// Canonical grouped-UI construction policy applied during propagation.
///
/// Source provenance (C++):
/// - `compiler/generator/compile.cpp`
/// - `compiler/generator/instructions_compiler.cpp`
///
/// Parity note:
/// - when the root UI group has an empty label, C++ rewrites it to the
///   canonical compilation name (top-level `declare name` or source stem)
///   before backend emission.
/// - Rust threads that canonical root label into grouped UI construction so
///   `UiProgram` is already the source of truth before FIR/backend lowering.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PropagateUiOptions {
    /// Canonical label used when propagation must synthesize or rename the root
    /// group.
    ///
    /// This should already reflect the C++ root-label policy:
    /// `declare name` from the master document when present, otherwise source
    /// filename stem.
    pub synthesized_root_label: Box<str>,
}

impl PropagateUiOptions {
    #[must_use]
    /// Creates one grouped-UI construction policy with the provided root label.
    pub fn new(synthesized_root_label: impl Into<Box<str>>) -> Self {
        Self {
            synthesized_root_label: synthesized_root_label.into(),
        }
    }
}

/// Typed handle for the flat post-eval box subset accepted at the propagation boundary.
///
/// # Source provenance (C++)
/// - `compiler/evaluate/eval.cpp`
/// - `evalprocess(...)`
/// - `a2sb(...)`
/// - `compiler/propagate/propagate.cpp`
/// - `realPropagate(...)`
///
/// The C++ production pipeline does not feed arbitrary box syntax into
/// propagation. It first evaluates the program and lowers residual
/// lambda/pattern forms through `a2sb(...)`, then propagates the resulting
/// first-order box tree.
///
/// Rust keeps the same tree arena and structural sharing model: `FlatBoxId` is
/// a validated `TreeId` wrapper, not a copied owned IR. Its role is to make the
/// post-eval contract explicit:
///
/// - accepted: flat first-order box families such as primitives, symbolic
///   slots, widgets, groups, route, and composition algebra,
/// - rejected: evaluator-only syntax that must have disappeared before
///   propagation, such as `abstr`, `case`, `pattern_var`, and local-definition
///   shells.
///
/// Validated flat-box handle used by the route-aware propagation fast path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FlatBoxId(TreeId);

impl FlatBoxId {
    #[must_use]
    pub fn as_tree_id(self) -> TreeId {
        self.0
    }

    fn from_tree_id(id: TreeId) -> Self {
        Self(id)
    }
}

/// Boundary validation error while converting a generic `BoxId` into a [`FlatBoxId`].
///
/// This error means the caller attempted to feed propagation a box family that
/// is not valid after the C++ `evalprocess -> a2sb` lowering boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
/// Errors returned while validating or decoding the flat-box subset.
pub enum FlatBoxBuildError {
    UnexpectedPostEvalBox { node: TreeId, kind: &'static str },
}

impl Display for FlatBoxBuildError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedPostEvalBox { node, kind } => write!(
                f,
                "box node {} ({kind}) is not valid in the post-eval flat box subset",
                node.as_u32()
            ),
        }
    }
}

impl std::error::Error for FlatBoxBuildError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Internal flat-box node classification used by the propagation fast path.
enum FlatNodeKind {
    Int,
    Real,
    Wire,
    Cut,
    Slot,
    Symbolic { body: FlatBoxId },
    Metadata { body: FlatBoxId },
    Prim1,
    Prim2,
    Prim3,
    Prim4,
    Prim5,
    FFun,
    FConst,
    FVar,
    Button,
    Checkbox,
    VSlider,
    HSlider,
    NumEntry,
    VBargraph,
    HBargraph,
    Soundfile,
    Waveform,
    VGroup { body: FlatBoxId },
    HGroup { body: FlatBoxId },
    TGroup { body: FlatBoxId },
    Seq(FlatBoxId, FlatBoxId),
    Par(FlatBoxId, FlatBoxId),
    Split(FlatBoxId, FlatBoxId),
    Merge(FlatBoxId, FlatBoxId),
    Rec(FlatBoxId, FlatBoxId),
    Environment,
    Route,
    Inputs,
    Outputs,
    Ondemand(FlatBoxId),
    Upsampling(FlatBoxId),
    Downsampling(FlatBoxId),
}

/// Validates that `root` belongs to the flat post-eval box subset and returns a typed handle.
///
/// This is a structural contract check only. It does not evaluate, simplify, or
/// normalize the tree. Callers should use it at the `eval/a2sb -> propagate`
/// boundary to guarantee that propagation never sees residual evaluator syntax.
pub fn try_build_flat_box(arena: &TreeArena, root: BoxId) -> Result<FlatBoxId, FlatBoxBuildError> {
    validate_flat_box(arena, root)
}

fn validate_flat_box(arena: &TreeArena, node: BoxId) -> Result<FlatBoxId, FlatBoxBuildError> {
    let flat = FlatBoxId::from_tree_id(node);
    let _ = flat_node_kind(arena, flat)?;
    Ok(flat)
}

fn flat_node_kind(arena: &TreeArena, node: FlatBoxId) -> Result<FlatNodeKind, FlatBoxBuildError> {
    let node_id = node.as_tree_id();
    match match_box(arena, node_id) {
        BoxMatch::Int(_) => Ok(FlatNodeKind::Int),
        BoxMatch::Real(_) => Ok(FlatNodeKind::Real),
        BoxMatch::Wire => Ok(FlatNodeKind::Wire),
        BoxMatch::Cut => Ok(FlatNodeKind::Cut),
        BoxMatch::Slot(_) => Ok(FlatNodeKind::Slot),
        BoxMatch::Symbolic(_, body) => Ok(FlatNodeKind::Symbolic {
            body: validate_flat_box(arena, body)?,
        }),
        BoxMatch::Metadata(body, _) => Ok(FlatNodeKind::Metadata {
            body: validate_flat_box(arena, body)?,
        }),
        BoxMatch::Add
        | BoxMatch::Sub
        | BoxMatch::Mul
        | BoxMatch::Div
        | BoxMatch::Rem
        | BoxMatch::And
        | BoxMatch::Or
        | BoxMatch::Xor
        | BoxMatch::Lsh
        | BoxMatch::Rsh
        | BoxMatch::Lt
        | BoxMatch::Le
        | BoxMatch::Gt
        | BoxMatch::Ge
        | BoxMatch::Eq
        | BoxMatch::Ne
        | BoxMatch::Pow
        | BoxMatch::Atan2
        | BoxMatch::Fmod
        | BoxMatch::Remainder
        | BoxMatch::Delay
        | BoxMatch::Min
        | BoxMatch::Max
        | BoxMatch::Prefix
        | BoxMatch::Attach
        | BoxMatch::Enable
        | BoxMatch::Control => Ok(FlatNodeKind::Prim2),
        BoxMatch::Delay1
        | BoxMatch::IntCast
        | BoxMatch::FloatCast
        | BoxMatch::Acos
        | BoxMatch::Asin
        | BoxMatch::Atan
        | BoxMatch::Cos
        | BoxMatch::Sin
        | BoxMatch::Tan
        | BoxMatch::Exp
        | BoxMatch::Log
        | BoxMatch::Log10
        | BoxMatch::Sqrt
        | BoxMatch::Abs
        | BoxMatch::Floor
        | BoxMatch::Ceil
        | BoxMatch::Rint
        | BoxMatch::Round
        | BoxMatch::Lowest
        | BoxMatch::Highest => Ok(FlatNodeKind::Prim1),
        BoxMatch::ReadOnlyTable | BoxMatch::Select2 | BoxMatch::AssertBounds => {
            Ok(FlatNodeKind::Prim3)
        }
        BoxMatch::Select3 => Ok(FlatNodeKind::Prim4),
        BoxMatch::WriteReadTable => Ok(FlatNodeKind::Prim5),
        BoxMatch::FFun(_) => Ok(FlatNodeKind::FFun),
        BoxMatch::FConst(_, _, _) => Ok(FlatNodeKind::FConst),
        BoxMatch::FVar(_, _, _) => Ok(FlatNodeKind::FVar),
        BoxMatch::Button(_) => Ok(FlatNodeKind::Button),
        BoxMatch::Checkbox(_) => Ok(FlatNodeKind::Checkbox),
        BoxMatch::VSlider(_, _, _, _, _) => Ok(FlatNodeKind::VSlider),
        BoxMatch::HSlider(_, _, _, _, _) => Ok(FlatNodeKind::HSlider),
        BoxMatch::NumEntry(_, _, _, _, _) => Ok(FlatNodeKind::NumEntry),
        BoxMatch::VBargraph(_, _, _) => Ok(FlatNodeKind::VBargraph),
        BoxMatch::HBargraph(_, _, _) => Ok(FlatNodeKind::HBargraph),
        BoxMatch::Soundfile(_, _) => Ok(FlatNodeKind::Soundfile),
        BoxMatch::Waveform(_) => Ok(FlatNodeKind::Waveform),
        BoxMatch::VGroup(_, body) => Ok(FlatNodeKind::VGroup {
            body: validate_flat_box(arena, body)?,
        }),
        BoxMatch::HGroup(_, body) => Ok(FlatNodeKind::HGroup {
            body: validate_flat_box(arena, body)?,
        }),
        BoxMatch::TGroup(_, body) => Ok(FlatNodeKind::TGroup {
            body: validate_flat_box(arena, body)?,
        }),
        BoxMatch::Seq(left, right) => Ok(FlatNodeKind::Seq(
            validate_flat_box(arena, left)?,
            validate_flat_box(arena, right)?,
        )),
        BoxMatch::Par(left, right) => Ok(FlatNodeKind::Par(
            validate_flat_box(arena, left)?,
            validate_flat_box(arena, right)?,
        )),
        BoxMatch::Split(left, right) => Ok(FlatNodeKind::Split(
            validate_flat_box(arena, left)?,
            validate_flat_box(arena, right)?,
        )),
        BoxMatch::Merge(left, right) => Ok(FlatNodeKind::Merge(
            validate_flat_box(arena, left)?,
            validate_flat_box(arena, right)?,
        )),
        BoxMatch::Rec(left, right) => Ok(FlatNodeKind::Rec(
            validate_flat_box(arena, left)?,
            validate_flat_box(arena, right)?,
        )),
        BoxMatch::Environment => Ok(FlatNodeKind::Environment),
        BoxMatch::Route(_, _, _) => Ok(FlatNodeKind::Route),
        BoxMatch::Inputs(_) => Ok(FlatNodeKind::Inputs),
        BoxMatch::Outputs(_) => Ok(FlatNodeKind::Outputs),
        BoxMatch::Ondemand(body) => Ok(FlatNodeKind::Ondemand(validate_flat_box(arena, body)?)),
        BoxMatch::Upsampling(body) => Ok(FlatNodeKind::Upsampling(validate_flat_box(arena, body)?)),
        BoxMatch::Downsampling(body) => {
            Ok(FlatNodeKind::Downsampling(validate_flat_box(arena, body)?))
        }
        BoxMatch::Ffunction(_, _, _) => Err(flat_box_unexpected(node_id, "ffunction")),
        BoxMatch::Unknown => Err(flat_box_unexpected(node_id, "unknown")),
        BoxMatch::Ident(_) => Err(flat_box_unexpected(node_id, "ident")),
        BoxMatch::Appl(_, _) => Err(flat_box_unexpected(node_id, "appl")),
        BoxMatch::Access(_, _) => Err(flat_box_unexpected(node_id, "access")),
        BoxMatch::IPar(_, _, _) => Err(flat_box_unexpected(node_id, "ipar")),
        BoxMatch::ISeq(_, _, _) => Err(flat_box_unexpected(node_id, "iseq")),
        BoxMatch::ISum(_, _, _) => Err(flat_box_unexpected(node_id, "isum")),
        BoxMatch::IProd(_, _, _) => Err(flat_box_unexpected(node_id, "iprod")),
        BoxMatch::WithLocalDef(_, _) => Err(flat_box_unexpected(node_id, "withlocaldef")),
        BoxMatch::ModifLocalDef(_, _) => Err(flat_box_unexpected(node_id, "modiflocaldef")),
        BoxMatch::WithRecDef(_, _, _) => Err(flat_box_unexpected(node_id, "withrecdef")),
        BoxMatch::Component(_) => Err(flat_box_unexpected(node_id, "component")),
        BoxMatch::Library(_) => Err(flat_box_unexpected(node_id, "library")),
        BoxMatch::Case(_) => Err(flat_box_unexpected(node_id, "case")),
        BoxMatch::PatternMatcher(_) => Err(flat_box_unexpected(node_id, "patternmatcher")),
        BoxMatch::Closure(_) => Err(flat_box_unexpected(node_id, "closure")),
        BoxMatch::PatternVar(_) => Err(flat_box_unexpected(node_id, "patternvar")),
        BoxMatch::Abstr(_, _) => Err(flat_box_unexpected(node_id, "abstr")),
        BoxMatch::Modulation(_, _) => Err(flat_box_unexpected(node_id, "modulation")),
    }
}

fn flat_box_unexpected(node: TreeId, kind: &'static str) -> FlatBoxBuildError {
    FlatBoxBuildError::UnexpectedPostEvalBox { node, kind }
}

/// Propagation/arity inference error.
#[derive(Clone, Debug, PartialEq, Eq)]
/// Errors returned by box-to-signal propagation.
pub enum PropagateError {
    UnsupportedBox {
        node: TreeId,
        kind: &'static str,
    },
    InvalidIntegerValue {
        node: TreeId,
        field: &'static str,
    },
    NegativeIntegerValue {
        field: &'static str,
        value: i64,
    },
    IntegerTooLarge {
        field: &'static str,
        value: usize,
    },
    InputArityMismatch {
        node: TreeId,
        expected: usize,
        got: usize,
    },
    OutputArityMismatch {
        node: TreeId,
        expected: usize,
        got: usize,
    },
    SeqArityMismatch {
        node: TreeId,
        left_outputs: usize,
        right_inputs: usize,
    },
    SplitArityMismatch {
        node: TreeId,
        left_outputs: usize,
        right_inputs: usize,
    },
    MergeArityMismatch {
        node: TreeId,
        left_outputs: usize,
        right_inputs: usize,
    },
    RecArityMismatch {
        node: TreeId,
        left_inputs: usize,
        left_outputs: usize,
        right_inputs: usize,
        right_outputs: usize,
    },
}

impl Display for PropagateError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedBox { node, kind } => {
                write!(f, "unsupported box node {} ({kind})", node.as_u32())
            }
            Self::InvalidIntegerValue { node, field } => {
                write!(
                    f,
                    "invalid integer value for `{field}` at node {}",
                    node.as_u32()
                )
            }
            Self::NegativeIntegerValue { field, value } => {
                write!(f, "negative integer value for `{field}`: {value}")
            }
            Self::IntegerTooLarge { field, value } => {
                write!(f, "integer value too large for `{field}`: {value}")
            }
            Self::InputArityMismatch {
                node,
                expected,
                got,
            } => write!(
                f,
                "input arity mismatch at node {}: expected {expected}, got {got}",
                node.as_u32()
            ),
            Self::OutputArityMismatch {
                node,
                expected,
                got,
            } => write!(
                f,
                "output arity mismatch at node {}: expected {expected}, got {got}",
                node.as_u32()
            ),
            Self::SeqArityMismatch {
                node,
                left_outputs,
                right_inputs,
            } => write!(
                f,
                "sequential composition mismatch at node {}: left outputs ({left_outputs}) != right inputs ({right_inputs})",
                node.as_u32()
            ),
            Self::SplitArityMismatch {
                node,
                left_outputs,
                right_inputs,
            } => write!(
                f,
                "split composition mismatch at node {}: left outputs ({left_outputs}) must divide right inputs ({right_inputs})",
                node.as_u32()
            ),
            Self::MergeArityMismatch {
                node,
                left_outputs,
                right_inputs,
            } => write!(
                f,
                "merge composition mismatch at node {}: left outputs ({left_outputs}) must be a multiple of right inputs ({right_inputs})",
                node.as_u32()
            ),
            Self::RecArityMismatch {
                node,
                left_inputs,
                left_outputs,
                right_inputs,
                right_outputs,
            } => write!(
                f,
                "recursive composition mismatch at node {}: right inputs ({right_inputs}) <= left outputs ({left_outputs}) and right outputs ({right_outputs}) <= left inputs ({left_inputs}) are required",
                node.as_u32()
            ),
        }
    }
}

impl std::error::Error for PropagateError {}

impl From<FlatBoxBuildError> for PropagateError {
    fn from(value: FlatBoxBuildError) -> Self {
        match value {
            FlatBoxBuildError::UnexpectedPostEvalBox { node, kind } => {
                Self::UnsupportedBox { node, kind }
            }
        }
    }
}

/// Converts propagation errors into structured diagnostics used by the compiler facade.
impl IntoDiagnostic for PropagateError {
    fn into_diagnostic(self) -> Diagnostic {
        let message = self.to_string();
        match self {
            Self::UnsupportedBox { .. } => {
                Diagnostic::new(Severity::Error, Stage::Propagate, codes::PROP_UNSUPPORTED_BOX, message)
                    .with_note(
                        "cause: encountered box node family is not supported in current propagation phase",
                    )
                    .with_help("evaluate box expression first or add propagation support for this node family")
            }
            Self::InputArityMismatch { expected, got, .. }
            | Self::OutputArityMismatch { expected, got, .. } => {
                Diagnostic::new(Severity::Error, Stage::Propagate, codes::PROP_ARITY_MISMATCH, message)
                    .with_note("cause: propagated bus width differs from required arity")
                    .with_note(
                        "rule: input/output arities at composition boundary must match target signature",
                    )
                    .with_note(format!("expected {expected}, got {got}"))
                    .with_help("adjust composition so input/output bus widths match")
            }
            Self::SeqArityMismatch {
                left_outputs,
                right_inputs,
                ..
            } => Diagnostic::new(Severity::Error, Stage::Propagate, codes::PROP_ARITY_MISMATCH, message)
                .with_note("cause: sequential composition bus widths do not match")
                .with_note("rule: seq(A, B) requires outputs(A) == inputs(B)")
                .with_note(format!(
                    "sequential composition requires left outputs ({left_outputs}) == right inputs ({right_inputs})"
                ))
                .with_note(format!(
                    "computed: {left_outputs} == {right_inputs} -> {}",
                    left_outputs == right_inputs
                ))
                .with_note(format!(
                    "suggested target: make outputs(A) and inputs(B) equal (common target: {})",
                    left_outputs.max(right_inputs)
                ))
                .with_help("for `A : B`, enforce outputs(A) == inputs(B)")
                .with_help("fix: adjust A or B channel count to same bus width")
                .with_help("template: process = A : B; // outputs(A) == inputs(B)"),
            Self::SplitArityMismatch {
                left_outputs,
                right_inputs,
                ..
            } => Diagnostic::new(Severity::Error, Stage::Propagate, codes::PROP_ARITY_MISMATCH, message)
                .with_note("cause: split composition divisibility rule is violated")
                .with_note("rule: split(A, B) requires inputs(B) % outputs(A) == 0")
                .with_note(format!(
                    "split composition requires right inputs ({right_inputs}) to be divisible by left outputs ({left_outputs})"
                ))
                .with_note(if left_outputs == 0 {
                    "computed: divisor outputs(A)=0 is invalid".to_owned()
                } else {
                    format!(
                        "computed: {right_inputs} % {left_outputs} = {}",
                        right_inputs % left_outputs
                    )
                })
                .with_note(if left_outputs == 0 {
                    "suggested target: outputs(A) must be > 0 before divisibility can be satisfied".to_owned()
                } else {
                    let next = right_inputs
                        .saturating_add(left_outputs - 1)
                        / left_outputs
                        * left_outputs;
                    format!(
                        "suggested target: set inputs(B) to {next} (next multiple of outputs(A)={left_outputs})"
                    )
                })
                .with_help("for `A <: B`, enforce inputs(B) % outputs(A) == 0")
                .with_help("fix: make B inputs a multiple of A outputs")
                .with_help("template: process = A <: B; // inputs(B) % outputs(A) == 0"),
            Self::MergeArityMismatch {
                left_outputs,
                right_inputs,
                ..
            } => Diagnostic::new(Severity::Error, Stage::Propagate, codes::PROP_ARITY_MISMATCH, message)
                .with_note("cause: merge composition multiple rule is violated")
                .with_note("rule: merge(A, B) requires outputs(A) % inputs(B) == 0")
                .with_note(format!(
                    "merge composition requires left outputs ({left_outputs}) to be a multiple of right inputs ({right_inputs})"
                ))
                .with_note(if right_inputs == 0 {
                    "computed: divisor inputs(B)=0 is invalid".to_owned()
                } else {
                    format!(
                        "computed: {left_outputs} % {right_inputs} = {}",
                        left_outputs % right_inputs
                    )
                })
                .with_note(if right_inputs == 0 {
                    "suggested target: inputs(B) must be > 0 before multiple constraints can be satisfied".to_owned()
                } else {
                    let next = left_outputs
                        .saturating_add(right_inputs - 1)
                        / right_inputs
                        * right_inputs;
                    format!(
                        "suggested target: set outputs(A) to {next} (next multiple of inputs(B)={right_inputs})"
                    )
                })
                .with_help("for `A :> B`, enforce outputs(A) % inputs(B) == 0")
                .with_help("fix: make A outputs a multiple of B inputs")
                .with_help("template: process = A :> B; // outputs(A) % inputs(B) == 0"),
            Self::RecArityMismatch {
                left_inputs,
                left_outputs,
                right_inputs,
                right_outputs,
                ..
            } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_RECURSION_MISMATCH,
                message,
            )
            .with_note("cause: recursive feedback arity constraints are not satisfied")
            .with_note(
                "rule: rec(A, B) requires right_inputs <= left_outputs and right_outputs <= left_inputs",
            )
            .with_note(format!(
                "required: right_inputs ({right_inputs}) <= left_outputs ({left_outputs}) and right_outputs ({right_outputs}) <= left_inputs ({left_inputs})"
            ))
            .with_note(format!(
                "computed: {} <= {} is {}, {} <= {} is {}",
                right_inputs,
                left_outputs,
                right_inputs <= left_outputs,
                right_outputs,
                left_inputs,
                right_outputs <= left_inputs
            ))
            .with_note(format!(
                "suggested target: set outputs(A) >= {} and inputs(A) >= {}",
                right_inputs, right_outputs
            ))
            .with_help(
                "for `A ~ B`, enforce inputs(B) <= outputs(A) and outputs(B) <= inputs(A)",
            )
            .with_help("fix: reduce B feedback bus or widen matching A arities")
            .with_help(
                "template: process = A ~ B; // inputs(B)<=outputs(A), outputs(B)<=inputs(A)",
            ),
            Self::InvalidIntegerValue { field, .. } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_GENERIC_FAILURE,
                message,
            )
            .with_note("cause: integer-valued propagation field has invalid runtime representation")
            .with_note(format!("invalid integer for field `{field}`")),
            Self::NegativeIntegerValue { field, value } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_GENERIC_FAILURE,
                message,
            )
            .with_note(
                "cause: field constrained to non-negative integer received a negative value",
            )
            .with_note(format!("field `{field}` is negative: {value}")),
            Self::IntegerTooLarge { field, value } => Diagnostic::new(
                Severity::Error,
                Stage::Propagate,
                codes::PROP_GENERIC_FAILURE,
                message,
            )
            .with_note("cause: integer field exceeds propagation-supported bounds")
            .with_note(format!("field `{field}` exceeds target range: {value}")),
        }
    }
}

/// Creates `n` canonical `sigInput(i)` signals.
///
/// Output order is stable and follows input bus index order: `0..n-1`.
#[must_use]
/// Builds the canonical list of `input(i)` signals for a given input arity.
pub fn make_sig_input_list(arena: &mut TreeArena, n: usize) -> Vec<SigId> {
    let mut b = SigBuilder::new(arena);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let index = i32::try_from(i).unwrap_or(i32::MAX);
        out.push(b.input(index));
    }
    out
}

/// Infers input/output arity of one flat post-eval box expression (memoized).
///
/// This is the typed entry point for post-`eval/a2sb` callers that already
/// hold a validated [`FlatBoxId`].
pub fn box_arity_typed(
    arena: &TreeArena,
    box_tree: FlatBoxId,
    cache: &mut ArityCache,
) -> Result<BoxArity, PropagateError> {
    if let Some(cached) = cache.get(&box_tree) {
        return cached.clone();
    }
    let result = box_arity_flat_inner(arena, box_tree, cache);
    cache.insert(box_tree, result.clone());
    result
}

#[doc(hidden)]
/// Computes box arity using the validated flat-box subset.
pub fn box_arity_flat(
    arena: &TreeArena,
    box_tree: FlatBoxId,
    cache: &mut ArityCache,
) -> Result<BoxArity, PropagateError> {
    box_arity_typed(arena, box_tree, cache)
}

/// Infers input/output arity of one box expression (memoized).
///
/// Compatibility wrapper for callers that still hold a raw [`BoxId`].
/// New post-`eval/a2sb` callers should prefer [`box_arity_typed`].
///
/// Callers should create one [`ArityCache`] and pass it through to amortise
/// repeated sub-expression visits across multiple calls.
pub fn box_arity(
    arena: &TreeArena,
    box_tree: BoxId,
    cache: &mut ArityCache,
) -> Result<BoxArity, PropagateError> {
    let flat = try_build_flat_box(arena, box_tree)?;
    box_arity_typed(arena, flat, cache)
}

/// Core arity inference logic, called only on cache miss.
fn box_arity_flat_inner(
    arena: &TreeArena,
    box_tree: FlatBoxId,
    cache: &mut ArityCache,
) -> Result<BoxArity, PropagateError> {
    match flat_node_kind(arena, box_tree)? {
        FlatNodeKind::Int | FlatNodeKind::Real => Ok(BoxArity {
            inputs: 0,
            outputs: 1,
        }),
        FlatNodeKind::Slot => Ok(BoxArity {
            inputs: 0,
            outputs: 1,
        }),
        FlatNodeKind::Metadata { body } => box_arity_typed(arena, body, cache),
        FlatNodeKind::Wire => Ok(BoxArity {
            inputs: 1,
            outputs: 1,
        }),
        FlatNodeKind::Cut => Ok(BoxArity {
            inputs: 1,
            outputs: 0,
        }),
        FlatNodeKind::Prim2 => Ok(BoxArity {
            inputs: 2,
            outputs: 1,
        }),
        FlatNodeKind::Prim1 => Ok(BoxArity {
            inputs: 1,
            outputs: 1,
        }),
        FlatNodeKind::Prim3 => Ok(BoxArity {
            inputs: 3,
            outputs: 1,
        }),
        FlatNodeKind::Prim4 => Ok(BoxArity {
            inputs: 4,
            outputs: 1,
        }),
        FlatNodeKind::Prim5 => Ok(BoxArity {
            inputs: 5,
            outputs: 1,
        }),
        FlatNodeKind::FConst | FlatNodeKind::FVar => Ok(BoxArity {
            inputs: 0,
            outputs: 1,
        }),
        FlatNodeKind::FFun => {
            let BoxMatch::FFun(ff) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat ffun node must decode to BoxMatch::FFun")
            };
            Ok(BoxArity {
                inputs: ffunction_arity(arena, ff)?,
                outputs: 1,
            })
        }
        FlatNodeKind::Button
        | FlatNodeKind::Checkbox
        | FlatNodeKind::VSlider
        | FlatNodeKind::HSlider
        | FlatNodeKind::NumEntry => Ok(BoxArity {
            inputs: 0,
            outputs: 1,
        }),
        FlatNodeKind::VBargraph | FlatNodeKind::HBargraph => Ok(BoxArity {
            inputs: 1,
            outputs: 1,
        }),
        FlatNodeKind::Soundfile => {
            let BoxMatch::Soundfile(_, chan) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat soundfile node must decode to BoxMatch::Soundfile")
            };
            let chan = usize_from_int_node(arena, chan, "soundfile channels")?;
            Ok(BoxArity {
                inputs: 2,
                outputs: 2 + chan,
            })
        }
        FlatNodeKind::Waveform => {
            let BoxMatch::Waveform(values) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat waveform node must decode to BoxMatch::Waveform")
            };
            let _ = list_length(arena, values).ok_or(PropagateError::UnsupportedBox {
                node: box_tree.as_tree_id(),
                kind: "waveform-list",
            })?;
            Ok(BoxArity {
                inputs: 0,
                outputs: 2,
            })
        }
        FlatNodeKind::VGroup { body }
        | FlatNodeKind::HGroup { body }
        | FlatNodeKind::TGroup { body } => box_arity_typed(arena, body, cache),
        FlatNodeKind::Symbolic { body } => {
            let inner = box_arity_typed(arena, body, cache)?;
            Ok(BoxArity {
                inputs: inner.inputs + 1,
                outputs: inner.outputs,
            })
        }
        FlatNodeKind::Seq(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            if left_arity.outputs != right_arity.inputs {
                return Err(PropagateError::SeqArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                });
            }
            Ok(BoxArity {
                inputs: left_arity.inputs,
                outputs: right_arity.outputs,
            })
        }
        FlatNodeKind::Par(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            Ok(BoxArity {
                inputs: left_arity.inputs + right_arity.inputs,
                outputs: left_arity.outputs + right_arity.outputs,
            })
        }
        FlatNodeKind::Split(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            if !split_compatible(left_arity.outputs, right_arity.inputs) {
                return Err(PropagateError::SplitArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                });
            }
            Ok(BoxArity {
                inputs: left_arity.inputs,
                outputs: right_arity.outputs,
            })
        }
        FlatNodeKind::Merge(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            if !merge_compatible(left_arity.outputs, right_arity.inputs) {
                return Err(PropagateError::MergeArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                });
            }
            Ok(BoxArity {
                inputs: left_arity.inputs,
                outputs: right_arity.outputs,
            })
        }
        FlatNodeKind::Rec(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            if right_arity.inputs > left_arity.outputs || right_arity.outputs > left_arity.inputs {
                return Err(PropagateError::RecArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_inputs: left_arity.inputs,
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                    right_outputs: right_arity.outputs,
                });
            }
            Ok(BoxArity {
                inputs: left_arity.inputs - right_arity.outputs,
                outputs: left_arity.outputs,
            })
        }
        FlatNodeKind::Environment => Ok(BoxArity {
            inputs: 0,
            outputs: 0,
        }),
        FlatNodeKind::Route => {
            let BoxMatch::Route(ins, outs, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat route node must decode to BoxMatch::Route")
            };
            Ok(BoxArity {
                inputs: usize_from_int_node(arena, ins, "route inputs")?,
                outputs: usize_from_int_node(arena, outs, "route outputs")?,
            })
        }
        FlatNodeKind::Inputs | FlatNodeKind::Outputs => Ok(BoxArity {
            inputs: 0,
            outputs: 1,
        }),
        FlatNodeKind::Ondemand(expr)
        | FlatNodeKind::Upsampling(expr)
        | FlatNodeKind::Downsampling(expr) => {
            let inner = box_arity_typed(arena, expr, cache)?;
            Ok(BoxArity {
                inputs: inner.inputs + 1,
                outputs: inner.outputs,
            })
        }
    }
}

/// Propagates input signals and grouped UI through one validated flat box expression.
///
/// This is the typed entry point for callers that already crossed the
/// `eval/a2sb` flat-box boundary and want the full propagation products:
/// propagated DSP signals plus canonical grouped UI ownership.
pub fn propagate_typed_with_ui(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
) -> Result<PropagateOutput, PropagateError> {
    propagate_typed_with_ui_options(
        arena,
        box_tree,
        inputs,
        cache,
        &PropagateUiOptions::default(),
    )
}

/// Propagates input signals and grouped UI through one validated flat box expression
/// using explicit grouped-UI construction options.
pub fn propagate_typed_with_ui_options(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
    ui_options: &PropagateUiOptions,
) -> Result<PropagateOutput, PropagateError> {
    let ui = build_ui_program(arena, box_tree, ui_options);
    let mut slot_env = SlotEnv::default();
    let clock_env = arena.nil();
    let signals = propagate_in_slot_env(
        arena,
        box_tree,
        inputs,
        cache,
        &ui.control_ids,
        &mut slot_env,
        clock_env,
    )?;
    Ok(PropagateOutput {
        signals,
        ui: ui.program,
    })
}

/// Propagates input signals through one validated flat box expression (memoized arity).
///
/// Compatibility wrapper for callers that only consume DSP signal outputs. New
/// post-`eval/a2sb` callers that own grouped UI should prefer
/// [`propagate_typed_with_ui`].
pub fn propagate_typed(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
) -> Result<Vec<SigId>, PropagateError> {
    propagate_typed_with_ui(arena, box_tree, inputs, cache).map(|output| output.signals)
}

/// Propagates input signals and grouped UI through one evaluated box expression.
///
/// Compatibility adapter for callers that still hold a raw [`BoxId`] but want
/// the explicit grouped UI artifact owned after propagation.
pub fn propagate_with_ui(
    arena: &mut TreeArena,
    box_tree: BoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
) -> Result<PropagateOutput, PropagateError> {
    let flat = try_build_flat_box(arena, box_tree)?;
    propagate_typed_with_ui(arena, flat, inputs, cache)
}

/// Propagates input signals through one evaluated box expression (memoized arity).
///
/// Compatibility wrapper for callers that still hold a raw [`BoxId`]. New
/// post-`eval/a2sb` callers should prefer [`propagate_typed`].
pub fn propagate(
    arena: &mut TreeArena,
    box_tree: BoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
) -> Result<Vec<SigId>, PropagateError> {
    let flat = try_build_flat_box(arena, box_tree)?;
    propagate_typed(arena, flat, inputs, cache)
}

/// Internal grouped-UI collector used while traversing a validated flat box.
///
/// This keeps UI ownership local to propagation:
/// - the UI tree is built in its own arena,
/// - controls are registered exactly once and assigned dense [`ControlId`]s,
/// - source widget/group labels are decoded before FIR/backend stages.
///
/// The `visited` cache deduplicates DAG traversal: the flat box tree after
/// `eval` is a **DAG** (the same `FlatBoxId` may be reachable via multiple
/// composition paths when the same variable/slider is used in several
/// positions).  Without deduplication each occurrence would create a ghost
/// `ControlSpec` entry while overwriting the `control_ids` mapping — producing
/// spurious slider fields in `buildUserInterface` that are never referenced
/// in the compute loop.  The cache is indexed by `FlatBoxId` so that any
/// subtree (not just widget leaves) is processed at most once.
struct UiCollector {
    builder: UiProgramBuilder,
    controls: Vec<ControlSpec>,
    control_ids: ControlIds,
    /// Memoisation table for the DAG walk — prevents re-visiting shared nodes.
    visited: AHashMap<FlatBoxId, UiCollectSummary>,
}

impl UiCollector {
    fn new() -> Self {
        Self {
            builder: UiProgramBuilder::new(),
            controls: Vec::new(),
            control_ids: ControlIds::default(),
            visited: AHashMap::default(),
        }
    }

    fn finish(self, options: &PropagateUiOptions) -> UiBuildOutput {
        let (mut arena, roots) = self.builder.finish();
        let keep_existing_root = matches!(roots.as_slice(), [only] if matches!(match_ui(&arena, *only), UiMatch::Group { .. }));
        let (root, root_origin) = if keep_existing_root {
            (
                rewrite_root_group_label(&mut arena, roots[0], options),
                UiRootOrigin::Explicit,
            )
        } else {
            (
                synthesize_ui_root_group(&mut arena, &options.synthesized_root_label, &roots),
                UiRootOrigin::Synthesized,
            )
        };
        UiBuildOutput {
            program: UiProgram {
                arena,
                root,
                controls: self.controls,
                root_origin,
                emit_ui: true,
            },
            control_ids: self.control_ids,
        }
    }

    fn register_control(
        &mut self,
        source_node: BoxId,
        kind: ControlKind,
        label: String,
        metadata: UiMetadata,
        range: Option<ControlRange>,
    ) -> ControlId {
        let id =
            ControlId::try_from(self.controls.len()).expect("control registry index fits in u32");
        self.controls.push(ControlSpec {
            id,
            kind,
            label,
            metadata,
            range,
        });
        self.control_ids.insert(source_node, id);
        id
    }

    fn input_control(
        &mut self,
        source_node: BoxId,
        path: &[UiGroupSpec],
        kind: ControlKind,
        label: String,
        metadata: UiMetadata,
        range: Option<ControlRange>,
    ) {
        let id = self.register_control(source_node, kind, label, metadata, range);
        self.builder.insert_input_control(path, id);
    }

    fn output_control(
        &mut self,
        source_node: BoxId,
        path: &[UiGroupSpec],
        kind: ControlKind,
        label: String,
        metadata: UiMetadata,
        range: Option<ControlRange>,
    ) {
        let id = self.register_control(source_node, kind, label, metadata, range);
        self.builder.insert_output_control(path, id);
    }

    fn soundfile(
        &mut self,
        source_node: BoxId,
        path: &[UiGroupSpec],
        label: String,
        metadata: UiMetadata,
    ) {
        let id = self.register_control(source_node, ControlKind::Soundfile, label, metadata, None);
        self.builder.insert_soundfile(path, id);
    }
}

fn synthesize_ui_root_group(arena: &mut TreeArena, label: &str, children: &[TreeId]) -> TreeId {
    ui::UiBuilder::new(arena).vgroup(label, children)
}

fn rewrite_root_group_label(
    arena: &mut TreeArena,
    root: TreeId,
    options: &PropagateUiOptions,
) -> TreeId {
    match match_ui(arena, root) {
        UiMatch::Group {
            kind,
            label,
            metadata,
            children,
        } if label.is_empty() && !options.synthesized_root_label.is_empty() => ui::UiBuilder::new(
            arena,
        )
        .group_with_metadata(kind, &options.synthesized_root_label, &metadata, &children),
        _ => root,
    }
}

/// Final products of grouped-UI extraction before signal lowering resumes.
///
/// `control_ids` is the bridge from source widget box nodes to stable
/// [`ControlId`]s embedded later in signal UI leaves.
struct UiBuildOutput {
    program: UiProgram,
    control_ids: ControlIds,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UiCollectSummary {
    has_ui: bool,
    preserve_ancestor_chain: bool,
}

/// Builds the canonical grouped-UI artifact for one validated flat box tree.
///
/// The returned [`UiProgram`] is already normalized for later phases:
/// - widget pathname labels have been rebased against the explicit group stack
///   like C++ `normalizePath(...)`,
/// - inline label metadata has been extracted,
/// - the root group has been synthesized or renamed according to
///   [`PropagateUiOptions`],
/// - every referenced control has a stable dense [`ControlId`].
fn build_ui_program(
    source_arena: &TreeArena,
    box_tree: FlatBoxId,
    options: &PropagateUiOptions,
) -> UiBuildOutput {
    let mut collector = UiCollector::new();
    let _ = collect_ui_nodes(source_arena, box_tree, &[], &mut collector);
    collector.finish(options)
}

/// Collects grouped UI nodes reachable from one validated flat box subtree.
///
/// Traversal follows the same semantic source tree used for DSP propagation,
/// but only UI-bearing families contribute concrete UI nodes:
/// widgets, bargraphs, soundfiles, and grouping wrappers. Composition-only DSP
/// nodes recurse structurally in deterministic source order.
///
/// Parity/adaptation note:
/// - widget labels still follow current C++ pathname rebasing,
/// - Rust additionally allows relative navigation on explicit group labels,
/// - placeholder explicit groups are kept only when their subtree contributes
///   UI or when a rebased explicit descendant needs the ancestor chain to stay
///   visible.
fn collect_ui_nodes(
    source_arena: &TreeArena,
    box_tree: FlatBoxId,
    current_groups: &[UiGroupPathSegment],
    collector: &mut UiCollector,
) -> UiCollectSummary {
    // DAG deduplication: the flat box tree after `eval` is a structural DAG —
    // the same arena node (e.g. a slider passed as a function argument) can be
    // reached from multiple composition paths.  Return the cached summary for
    // any node already processed; this prevents ghost ControlSpec registrations
    // and ensures each logical widget appears exactly once in the UI program.
    if let Some(&cached) = collector.visited.get(&box_tree) {
        return cached;
    }

    let kind = flat_node_kind(source_arena, box_tree).expect("validated flat box must decode");
    let result = match kind {
        FlatNodeKind::Button => {
            let BoxMatch::Button(label) = match_box(source_arena, box_tree.as_tree_id()) else {
                unreachable!("flat button node must decode to BoxMatch::Button")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.input_control(
                box_tree.as_tree_id(),
                &path,
                ControlKind::Button,
                label,
                metadata,
                None,
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::Checkbox => {
            let BoxMatch::Checkbox(label) = match_box(source_arena, box_tree.as_tree_id()) else {
                unreachable!("flat checkbox node must decode to BoxMatch::Checkbox")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.input_control(
                box_tree.as_tree_id(),
                &path,
                ControlKind::Checkbox,
                label,
                metadata,
                None,
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::VSlider => {
            let BoxMatch::VSlider(label, init, min, max, step) =
                match_box(source_arena, box_tree.as_tree_id())
            else {
                unreachable!("flat vslider node must decode to BoxMatch::VSlider")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.input_control(
                box_tree.as_tree_id(),
                &path,
                ControlKind::VSlider,
                label,
                metadata,
                Some(ControlRange {
                    init: decode_box_scalar(source_arena, init),
                    min: decode_box_scalar(source_arena, min),
                    max: decode_box_scalar(source_arena, max),
                    step: decode_box_scalar(source_arena, step),
                }),
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::HSlider => {
            let BoxMatch::HSlider(label, init, min, max, step) =
                match_box(source_arena, box_tree.as_tree_id())
            else {
                unreachable!("flat hslider node must decode to BoxMatch::HSlider")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.input_control(
                box_tree.as_tree_id(),
                &path,
                ControlKind::HSlider,
                label,
                metadata,
                Some(ControlRange {
                    init: decode_box_scalar(source_arena, init),
                    min: decode_box_scalar(source_arena, min),
                    max: decode_box_scalar(source_arena, max),
                    step: decode_box_scalar(source_arena, step),
                }),
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::NumEntry => {
            let BoxMatch::NumEntry(label, init, min, max, step) =
                match_box(source_arena, box_tree.as_tree_id())
            else {
                unreachable!("flat numentry node must decode to BoxMatch::NumEntry")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.input_control(
                box_tree.as_tree_id(),
                &path,
                ControlKind::NumEntry,
                label,
                metadata,
                Some(ControlRange {
                    init: decode_box_scalar(source_arena, init),
                    min: decode_box_scalar(source_arena, min),
                    max: decode_box_scalar(source_arena, max),
                    step: decode_box_scalar(source_arena, step),
                }),
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::VBargraph => {
            let BoxMatch::VBargraph(label, min, max) =
                match_box(source_arena, box_tree.as_tree_id())
            else {
                unreachable!("flat vbargraph node must decode to BoxMatch::VBargraph")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.output_control(
                box_tree.as_tree_id(),
                &path,
                ControlKind::VBargraph,
                label,
                metadata,
                Some(ControlRange {
                    init: 0.0,
                    min: decode_box_scalar(source_arena, min),
                    max: decode_box_scalar(source_arena, max),
                    step: 0.0,
                }),
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::HBargraph => {
            let BoxMatch::HBargraph(label, min, max) =
                match_box(source_arena, box_tree.as_tree_id())
            else {
                unreachable!("flat hbargraph node must decode to BoxMatch::HBargraph")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.output_control(
                box_tree.as_tree_id(),
                &path,
                ControlKind::HBargraph,
                label,
                metadata,
                Some(ControlRange {
                    init: 0.0,
                    min: decode_box_scalar(source_arena, min),
                    max: decode_box_scalar(source_arena, max),
                    step: 0.0,
                }),
            );
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::Soundfile => {
            let BoxMatch::Soundfile(label, _) = match_box(source_arena, box_tree.as_tree_id())
            else {
                unreachable!("flat soundfile node must decode to BoxMatch::Soundfile")
            };
            let normalized =
                normalize_widget_label_path(&decode_box_label(source_arena, label), current_groups);
            let path = canonical_group_path(&normalized.groups);
            let (label, metadata) = split_label_metadata(&normalized.raw_label);
            collector.soundfile(box_tree.as_tree_id(), &path, label, metadata);
            UiCollectSummary {
                has_ui: true,
                preserve_ancestor_chain: false,
            }
        }
        FlatNodeKind::VGroup { body } => collect_group_ui(
            source_arena,
            body,
            current_groups,
            collector,
            UiGroupKind::Vertical,
            box_tree.as_tree_id(),
        ),
        FlatNodeKind::HGroup { body } => collect_group_ui(
            source_arena,
            body,
            current_groups,
            collector,
            UiGroupKind::Horizontal,
            box_tree.as_tree_id(),
        ),
        FlatNodeKind::TGroup { body } => collect_group_ui(
            source_arena,
            body,
            current_groups,
            collector,
            UiGroupKind::Tab,
            box_tree.as_tree_id(),
        ),
        FlatNodeKind::Symbolic { body }
        | FlatNodeKind::Metadata { body }
        | FlatNodeKind::Ondemand(body)
        | FlatNodeKind::Upsampling(body)
        | FlatNodeKind::Downsampling(body) => {
            collect_ui_nodes(source_arena, body, current_groups, collector)
        }
        FlatNodeKind::Seq(left, right)
        | FlatNodeKind::Par(left, right)
        | FlatNodeKind::Split(left, right)
        | FlatNodeKind::Merge(left, right)
        | FlatNodeKind::Rec(left, right) => {
            let left_summary = collect_ui_nodes(source_arena, left, current_groups, collector);
            let right_summary = collect_ui_nodes(source_arena, right, current_groups, collector);
            UiCollectSummary {
                has_ui: left_summary.has_ui || right_summary.has_ui,
                preserve_ancestor_chain: left_summary.preserve_ancestor_chain
                    || right_summary.preserve_ancestor_chain,
            }
        }
        FlatNodeKind::Int
        | FlatNodeKind::Real
        | FlatNodeKind::Wire
        | FlatNodeKind::Cut
        | FlatNodeKind::Slot
        | FlatNodeKind::Prim1
        | FlatNodeKind::Prim2
        | FlatNodeKind::Prim3
        | FlatNodeKind::Prim4
        | FlatNodeKind::Prim5
        | FlatNodeKind::FFun
        | FlatNodeKind::FConst
        | FlatNodeKind::FVar
        | FlatNodeKind::Waveform
        | FlatNodeKind::Environment
        | FlatNodeKind::Route
        | FlatNodeKind::Inputs
        | FlatNodeKind::Outputs => UiCollectSummary::default(),
    };
    collector.visited.insert(box_tree, result);
    result
}

fn collect_group_ui(
    source_arena: &TreeArena,
    body: FlatBoxId,
    current_groups: &[UiGroupPathSegment],
    collector: &mut UiCollector,
    kind: UiGroupKind,
    group_node: BoxId,
) -> UiCollectSummary {
    let label = match match_box(source_arena, group_node) {
        BoxMatch::VGroup(label, _) | BoxMatch::HGroup(label, _) | BoxMatch::TGroup(label, _) => {
            decode_box_label(source_arena, label)
        }
        _ => unreachable!("flat group node must decode to a group box"),
    };
    let normalized = normalize_group_label_navigation(&label, current_groups, kind);
    let mut nested_groups = normalized.parent_groups;
    nested_groups.push(normalized.group);

    let path = canonical_group_path(&nested_groups);
    let terminal_preexisting = collector.builder.find_group_path(&path);
    let terminal = collector
        .builder
        .ensure_group_path(&path)
        .expect("explicit group path must yield a terminal group");

    let summary = collect_ui_nodes(source_arena, body, &nested_groups, collector);
    let keep_group =
        collector.builder.group_has_children(terminal) || summary.preserve_ancestor_chain;
    if !keep_group && terminal_preexisting.is_none() {
        let removed = collector.builder.remove_group_if_empty(terminal);
        debug_assert!(
            removed,
            "fresh explicit group placeholder should be removable"
        );
    }

    UiCollectSummary {
        has_ui: summary.has_ui,
        preserve_ancestor_chain: keep_group,
    }
}

/// Converts one raw explicit-group stack into its canonical stored UI path.
///
/// Metadata extraction happens after pathname normalization so segments such as
/// `../gain [style:knob]` first rebase to the correct group and only then split
/// the final widget label and group metadata.
fn canonical_group_path(path: &[UiGroupPathSegment]) -> Vec<UiGroupSpec> {
    path.iter().map(canonicalize_group_spec).collect()
}

fn decode_box_label(arena: &TreeArena, node: BoxId) -> String {
    if let BoxMatch::Ident(value) = match_box(arena, node) {
        return value.to_string();
    }
    match arena.kind(node) {
        Some(NodeKind::StringLiteral(value)) | Some(NodeKind::Symbol(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn decode_box_scalar(arena: &TreeArena, node: BoxId) -> f64 {
    match match_box(arena, node) {
        BoxMatch::Int(value) => f64::from(value),
        BoxMatch::Real(value) => value,
        _ => 0.0,
    }
}

/// Propagates one box tree with an explicit slot environment.
///
/// Source provenance (C++):
/// - `compiler/propagate/propagate.cpp`
/// - `propagate(...)`
///
/// C++ threads a dedicated `slotenv` alongside the normal recursion so
/// `boxSymbolic(slot, body)` can bind the first input bus to `boxSlot(slot)`.
/// Rust keeps the same semantic mechanism but uses a local hash map keyed by the
/// canonical `BoxId` of each slot node instead of global tree properties.
///
/// This helper is also the point where Rust enforces the public
/// `propagate(...)` contract: callers may only enter `propagate_inner(...)`
/// through a path that has already checked both input and output bus widths.
fn propagate_in_slot_env(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
    control_ids: &ControlIds,
    slot_env: &mut SlotEnv,
    clock_env: TreeId,
) -> Result<Vec<SigId>, PropagateError> {
    let arity = box_arity_typed(arena, box_tree, cache)?;
    if inputs.len() != arity.inputs {
        return Err(PropagateError::InputArityMismatch {
            node: box_tree.as_tree_id(),
            expected: arity.inputs,
            got: inputs.len(),
        });
    }
    let outputs = propagate_inner(
        arena,
        box_tree,
        inputs,
        cache,
        control_ids,
        slot_env,
        clock_env,
    )?;
    if outputs.len() != arity.outputs {
        return Err(PropagateError::OutputArityMismatch {
            node: box_tree.as_tree_id(),
            expected: arity.outputs,
            got: outputs.len(),
        });
    }
    Ok(outputs)
}

/// Internal propagation dispatcher once input arity has been validated.
///
/// Unlike [`box_arity_typed`], this function is intentionally operational rather
/// than declarative: it builds actual signal nodes, threads slot bindings, and
/// recursively performs composition rewrites. Unsupported box families here are
/// therefore genuine lowering gaps, not just missing arity metadata.
fn propagate_inner(
    arena: &mut TreeArena,
    box_tree: FlatBoxId,
    inputs: &[SigId],
    cache: &mut ArityCache,
    control_ids: &ControlIds,
    slot_env: &mut SlotEnv,
    clock_env: TreeId,
) -> Result<Vec<SigId>, PropagateError> {
    match flat_node_kind(arena, box_tree)? {
        FlatNodeKind::Int => {
            let BoxMatch::Int(value) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat int node must decode to BoxMatch::Int")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.int(value)])
        }
        FlatNodeKind::Real => {
            let BoxMatch::Real(value) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat real node must decode to BoxMatch::Real")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.real(value)])
        }
        FlatNodeKind::Metadata { body } => {
            propagate_inner(arena, body, inputs, cache, control_ids, slot_env, clock_env)
        }
        FlatNodeKind::Slot => {
            let BoxMatch::Slot(id) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat slot node must decode to BoxMatch::Slot")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            if let Some(sig) = slot_env.get(&box_tree.as_tree_id()).copied() {
                Ok(vec![sig])
            } else {
                let mut b = SigBuilder::new(arena);
                Ok(vec![b.input(id)])
            }
        }
        FlatNodeKind::Wire => {
            expect_input_arity(box_tree.as_tree_id(), inputs, 1)?;
            Ok(vec![inputs[0]])
        }
        FlatNodeKind::Cut => {
            expect_input_arity(box_tree.as_tree_id(), inputs, 1)?;
            Ok(Vec::new())
        }
        FlatNodeKind::Prim2 => {
            let op = match_box(arena, box_tree.as_tree_id());
            match op {
                BoxMatch::Add => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.add(x, y))
                }
                BoxMatch::Sub => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.sub(x, y))
                }
                BoxMatch::Mul => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.mul(x, y))
                }
                BoxMatch::Div => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.div(x, y))
                }
                BoxMatch::Rem => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.rem(x, y))
                }
                BoxMatch::And => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.and(x, y))
                }
                BoxMatch::Or => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.or(x, y))
                }
                BoxMatch::Xor => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.xor(x, y))
                }
                BoxMatch::Lsh => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.lsh(x, y))
                }
                BoxMatch::Rsh => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.arsh(x, y))
                }
                BoxMatch::Lt => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.lt(x, y))
                }
                BoxMatch::Le => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.le(x, y))
                }
                BoxMatch::Gt => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.gt(x, y))
                }
                BoxMatch::Ge => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.ge(x, y))
                }
                BoxMatch::Eq => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.eq(x, y))
                }
                BoxMatch::Ne => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.ne(x, y))
                }
                BoxMatch::Pow => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.pow(x, y))
                }
                BoxMatch::Atan2 => binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                    b.atan2(x, y)
                }),
                BoxMatch::Fmod => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.fmod(x, y))
                }
                BoxMatch::Remainder => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                        b.remainder(x, y)
                    })
                }
                BoxMatch::Min => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.min(x, y))
                }
                BoxMatch::Max => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| b.max(x, y))
                }
                BoxMatch::Delay => binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                    b.delay(x, y)
                }),
                BoxMatch::Prefix => binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                    b.prefix(x, y)
                }),
                BoxMatch::Attach => binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                    b.attach(x, y)
                }),
                BoxMatch::Enable => binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                    b.enable(x, y)
                }),
                BoxMatch::Control => {
                    binary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y| {
                        b.control(x, y)
                    })
                }
                _ => unreachable!("flat prim2 node must decode to a binary primitive"),
            }
        }
        FlatNodeKind::Prim1 => {
            let op = match_box(arena, box_tree.as_tree_id());
            match op {
                BoxMatch::Delay1 => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.delay1(x))
                }
                BoxMatch::IntCast => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.int_cast(x))
                }
                BoxMatch::FloatCast => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.float_cast(x))
                }
                BoxMatch::Acos => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.acos(x))
                }
                BoxMatch::Asin => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.asin(x))
                }
                BoxMatch::Atan => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.atan(x))
                }
                BoxMatch::Cos => unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.cos(x)),
                BoxMatch::Sin => unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.sin(x)),
                BoxMatch::Tan => unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.tan(x)),
                BoxMatch::Exp => unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.exp(x)),
                BoxMatch::Log => unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.log(x)),
                BoxMatch::Log10 => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.log10(x))
                }
                BoxMatch::Sqrt => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.sqrt(x))
                }
                BoxMatch::Abs => unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.abs(x)),
                BoxMatch::Floor => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.floor(x))
                }
                BoxMatch::Ceil => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.ceil(x))
                }
                BoxMatch::Rint => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.rint(x))
                }
                BoxMatch::Round => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.round(x))
                }
                BoxMatch::Lowest => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.lowest(x))
                }
                BoxMatch::Highest => {
                    unary_prim(arena, box_tree.as_tree_id(), inputs, |b, x| b.highest(x))
                }
                _ => unreachable!("flat prim1 node must decode to a unary primitive"),
            }
        }
        FlatNodeKind::Prim3 => {
            let op = match_box(arena, box_tree.as_tree_id());
            match op {
                BoxMatch::ReadOnlyTable => {
                    ternary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y, z| {
                        b.read_only_table(x, y, z)
                    })
                }
                BoxMatch::Select2 => {
                    ternary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y, z| {
                        b.select2(x, y, z)
                    })
                }
                BoxMatch::AssertBounds => {
                    ternary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y, z| {
                        b.assert_bounds(x, y, z)
                    })
                }
                _ => unreachable!("flat prim3 node must decode to a ternary primitive"),
            }
        }
        FlatNodeKind::Prim4 => {
            quaternary_prim(arena, box_tree.as_tree_id(), inputs, |b, x, y, z, w| {
                b.select3(x, y, z, w)
            })
        }
        FlatNodeKind::Prim5 => quinary_prim(
            arena,
            box_tree.as_tree_id(),
            inputs,
            |b, s, i, wi, ws, ri| b.write_read_table(s, i, wi, ws, ri),
        ),
        FlatNodeKind::FConst => {
            let BoxMatch::FConst(ty, name, file) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat fconst node must decode to BoxMatch::FConst")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.fconst(ty, name, file)])
        }
        FlatNodeKind::FVar => {
            let BoxMatch::FVar(ty, name, file) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat fvar node must decode to BoxMatch::FVar")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.fvar(ty, name, file)])
        }
        FlatNodeKind::Button => {
            let BoxMatch::Button(_) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat button node must decode to BoxMatch::Button")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let control = *control_ids
                .get(&box_tree.as_tree_id())
                .expect("button control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.button(control)])
        }
        FlatNodeKind::Checkbox => {
            let BoxMatch::Checkbox(_) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat checkbox node must decode to BoxMatch::Checkbox")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let control = *control_ids
                .get(&box_tree.as_tree_id())
                .expect("checkbox control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.checkbox(control)])
        }
        FlatNodeKind::VSlider => {
            let BoxMatch::VSlider(_, _, _, _, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat vslider node must decode to BoxMatch::VSlider")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let control = *control_ids
                .get(&box_tree.as_tree_id())
                .expect("vslider control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.vslider(control)])
        }
        FlatNodeKind::HSlider => {
            let BoxMatch::HSlider(_, _, _, _, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat hslider node must decode to BoxMatch::HSlider")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let control = *control_ids
                .get(&box_tree.as_tree_id())
                .expect("hslider control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.hslider(control)])
        }
        FlatNodeKind::NumEntry => {
            let BoxMatch::NumEntry(_, _, _, _, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat numentry node must decode to BoxMatch::NumEntry")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let control = *control_ids
                .get(&box_tree.as_tree_id())
                .expect("numentry control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.numentry(control)])
        }
        FlatNodeKind::VBargraph => {
            let BoxMatch::VBargraph(_, _, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat vbargraph node must decode to BoxMatch::VBargraph")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 1)?;
            let control = *control_ids
                .get(&box_tree.as_tree_id())
                .expect("vbargraph control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.vbargraph(control, inputs[0])])
        }
        FlatNodeKind::HBargraph => {
            let BoxMatch::HBargraph(_, _, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat hbargraph node must decode to BoxMatch::HBargraph")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 1)?;
            let control = *control_ids
                .get(&box_tree.as_tree_id())
                .expect("hbargraph control id must be registered during UI extraction");
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.hbargraph(control, inputs[0])])
        }
        FlatNodeKind::Waveform => {
            let BoxMatch::Waveform(values) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat waveform node must decode to BoxMatch::Waveform")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let values = list_to_vec(arena, values).ok_or(PropagateError::UnsupportedBox {
                node: box_tree.as_tree_id(),
                kind: "waveform-list",
            })?;
            let mut b = SigBuilder::new(arena);
            let n = i32_from_usize(values.len(), "waveform size")?;
            let size = b.int(n);
            let waveform = b.waveform(&values);
            Ok(vec![size, waveform])
        }
        FlatNodeKind::VGroup { body }
        | FlatNodeKind::HGroup { body }
        | FlatNodeKind::TGroup { body } => {
            propagate_in_slot_env(arena, body, inputs, cache, control_ids, slot_env, clock_env)
        }
        FlatNodeKind::Symbolic { body } => {
            let BoxMatch::Symbolic(slot, _) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat symbolic node must decode to BoxMatch::Symbolic")
            };
            if inputs.is_empty() {
                return Err(PropagateError::InputArityMismatch {
                    node: box_tree.as_tree_id(),
                    expected: 1,
                    got: 0,
                });
            }
            let previous = slot_env.insert(slot, inputs[0]);
            let result = propagate_in_slot_env(
                arena,
                body,
                &inputs[1..],
                cache,
                control_ids,
                slot_env,
                clock_env,
            );
            if let Some(sig) = previous {
                slot_env.insert(slot, sig);
            } else {
                slot_env.remove(&slot);
            }
            result
        }
        FlatNodeKind::Seq(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            if left_arity.outputs != right_arity.inputs {
                return Err(PropagateError::SeqArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                });
            }
            let mid = propagate_in_slot_env(
                arena,
                left,
                inputs,
                cache,
                control_ids,
                slot_env,
                clock_env,
            )?;
            propagate_in_slot_env(arena, right, &mid, cache, control_ids, slot_env, clock_env)
        }
        FlatNodeKind::Par(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            let left_out = propagate_in_slot_env(
                arena,
                left,
                &inputs[..left_arity.inputs],
                cache,
                control_ids,
                slot_env,
                clock_env,
            )?;
            let mut right_out = propagate_in_slot_env(
                arena,
                right,
                &inputs[left_arity.inputs..left_arity.inputs + right_arity.inputs],
                cache,
                control_ids,
                slot_env,
                clock_env,
            )?;
            let mut out = left_out;
            out.append(&mut right_out);
            Ok(out)
        }
        FlatNodeKind::Split(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            if !split_compatible(left_arity.outputs, right_arity.inputs) {
                return Err(PropagateError::SplitArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                });
            }
            let left_out = propagate_in_slot_env(
                arena,
                left,
                inputs,
                cache,
                control_ids,
                slot_env,
                clock_env,
            )?;
            let split_in = split_signals(&left_out, right_arity.inputs);
            propagate_in_slot_env(
                arena,
                right,
                &split_in,
                cache,
                control_ids,
                slot_env,
                clock_env,
            )
        }
        FlatNodeKind::Merge(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            if !merge_compatible(left_arity.outputs, right_arity.inputs) {
                return Err(PropagateError::MergeArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                });
            }
            let left_out = propagate_in_slot_env(
                arena,
                left,
                inputs,
                cache,
                control_ids,
                slot_env,
                clock_env,
            )?;
            let merge_in = mix_signals(arena, &left_out, right_arity.inputs);
            propagate_in_slot_env(
                arena,
                right,
                &merge_in,
                cache,
                control_ids,
                slot_env,
                clock_env,
            )
        }
        FlatNodeKind::Rec(left, right) => {
            let left_arity = box_arity_typed(arena, left, cache)?;
            let right_arity = box_arity_typed(arena, right, cache)?;
            if right_arity.inputs > left_arity.outputs || right_arity.outputs > left_arity.inputs {
                return Err(PropagateError::RecArityMismatch {
                    node: box_tree.as_tree_id(),
                    left_inputs: left_arity.inputs,
                    left_outputs: left_arity.outputs,
                    right_inputs: right_arity.inputs,
                    right_outputs: right_arity.outputs,
                });
            }

            let l0 = make_mem_sig_proj_list(arena, right_arity.inputs)?;
            let l1 =
                propagate_in_slot_env(arena, right, &l0, cache, control_ids, slot_env, clock_env)?;

            let mut rec_inputs = l1;
            rec_inputs.extend(lift_signals(arena, inputs));

            let l2 = propagate_in_slot_env(
                arena,
                left,
                &rec_inputs,
                cache,
                control_ids,
                slot_env,
                clock_env,
            )?;
            let group_body = vec_to_list(arena, &l2);
            let group = debruijn_rec(arena, group_body);

            let mut outputs = Vec::with_capacity(l2.len());
            let mut aperture_memo: AHashMap<TreeId, i64> = AHashMap::default();
            for (index, expr) in l2.iter().copied().enumerate() {
                if aperture(arena, expr, &mut aperture_memo) > 0 {
                    let idx = i32_from_usize(index, "rec projection index")?;
                    let mut b = SigBuilder::new(arena);
                    outputs.push(b.proj(idx, group));
                } else {
                    outputs.push(expr);
                }
            }
            Ok(outputs)
        }
        FlatNodeKind::Inputs => {
            let BoxMatch::Inputs(expr) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat inputs node must decode to BoxMatch::Inputs")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let arity = box_arity(arena, expr, cache)?;
            let value = i32_from_usize(arity.inputs, "inputs")?;
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.int(value)])
        }
        FlatNodeKind::Outputs => {
            let BoxMatch::Outputs(expr) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat outputs node must decode to BoxMatch::Outputs")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            let arity = box_arity(arena, expr, cache)?;
            let value = i32_from_usize(arity.outputs, "outputs")?;
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.int(value)])
        }
        FlatNodeKind::Environment => {
            expect_input_arity(box_tree.as_tree_id(), inputs, 0)?;
            Ok(Vec::new())
        }
        FlatNodeKind::Route => {
            let BoxMatch::Route(ins, outs, route_spec) = match_box(arena, box_tree.as_tree_id())
            else {
                unreachable!("flat route node must decode to BoxMatch::Route")
            };
            let input_count = usize_from_int_node(arena, ins, "route inputs")?;
            let output_count = usize_from_int_node(arena, outs, "route outputs")?;
            expect_input_arity(box_tree.as_tree_id(), inputs, input_count)?;

            let route = flatten_route_ints(arena, route_spec)?;
            let mut b = SigBuilder::new(arena);
            let zero = b.int(0);
            let mut outputs = vec![zero; output_count];

            for pair in route.chunks_exact(2) {
                let src = pair[0];
                let dst = pair[1];
                if dst <= 0 {
                    continue;
                }
                let Ok(dst_index) = usize::try_from(dst - 1) else {
                    continue;
                };
                if dst_index >= output_count || src <= 0 {
                    continue;
                }
                let Ok(src_index) = usize::try_from(src - 1) else {
                    continue;
                };
                if src_index >= input_count {
                    continue;
                }
                outputs[dst_index] = b.add(outputs[dst_index], inputs[src_index]);
            }

            Ok(outputs)
        }
        FlatNodeKind::FFun => {
            let BoxMatch::FFun(ff) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat ffun node must decode to BoxMatch::FFun")
            };
            let expected = ffunction_arity(arena, ff)?;
            expect_input_arity(box_tree.as_tree_id(), inputs, expected)?;
            let args = vec_to_list(arena, inputs);
            let mut b = SigBuilder::new(arena);
            Ok(vec![b.ffun(ff, args)])
        }
        FlatNodeKind::Soundfile => {
            let BoxMatch::Soundfile(_, chan) = match_box(arena, box_tree.as_tree_id()) else {
                unreachable!("flat soundfile node must decode to BoxMatch::Soundfile")
            };
            expect_input_arity(box_tree.as_tree_id(), inputs, 2)?;
            let chan_count = usize_from_int_node(arena, chan, "soundfile channels")?;
            let mut b = SigBuilder::new(arena);
            let control = *control_ids
                .get(&box_tree.as_tree_id())
                .expect("soundfile control id must be registered during UI extraction");
            let soundfile = b.soundfile(control);
            let part = inputs[0];
            let length = b.soundfile_length(soundfile, part);
            let rate = b.soundfile_rate(soundfile, part);
            let one = b.int(1);
            let zero = b.int(0);
            let upper = b.sub(length, one);
            let limited = b.min(inputs[1], upper);
            let clamped = b.max(zero, limited);

            let mut outputs = Vec::with_capacity(chan_count + 2);
            outputs.push(length);
            outputs.push(rate);
            for chan_index in 0..chan_count {
                let chan_sig = b.int(i32_from_usize(chan_index, "soundfile buffer channel")?);
                outputs.push(b.soundfile_buffer(soundfile, chan_sig, part, clamped));
            }
            Ok(outputs)
        }
        FlatNodeKind::Ondemand(body) => propagate_clocked_wrapper(
            arena,
            box_tree,
            body,
            inputs,
            ClockedWrapperCtx {
                cache,
                control_ids,
                slot_env,
                clock_env,
            },
            ClockedWrapperKind::Ondemand,
        ),
        FlatNodeKind::Upsampling(body) => propagate_clocked_wrapper(
            arena,
            box_tree,
            body,
            inputs,
            ClockedWrapperCtx {
                cache,
                control_ids,
                slot_env,
                clock_env,
            },
            ClockedWrapperKind::Upsampling,
        ),
        FlatNodeKind::Downsampling(body) => propagate_clocked_wrapper(
            arena,
            box_tree,
            body,
            inputs,
            ClockedWrapperCtx {
                cache,
                control_ids,
                slot_env,
                clock_env,
            },
            ClockedWrapperKind::Downsampling,
        ),
    }
}

#[derive(Clone, Copy)]
/// Clocked-wrapper categories recognized during propagation.
enum ClockedWrapperKind {
    Ondemand,
    Upsampling,
    Downsampling,
}

/// Context carried while lowering clocked wrapper patterns.
struct ClockedWrapperCtx<'a> {
    cache: &'a mut ArityCache,
    control_ids: &'a ControlIds,
    slot_env: &'a mut SlotEnv,
    clock_env: TreeId,
}

fn propagate_clocked_wrapper(
    arena: &mut TreeArena,
    wrapper_node: FlatBoxId,
    body: FlatBoxId,
    inputs: &[SigId],
    ctx: ClockedWrapperCtx<'_>,
    kind: ClockedWrapperKind,
) -> Result<Vec<SigId>, PropagateError> {
    let Some((&clock, tail)) = inputs.split_first() else {
        return Err(PropagateError::InputArityMismatch {
            node: wrapper_node.as_tree_id(),
            expected: 1,
            got: 0,
        });
    };

    let ClockedWrapperCtx {
        cache,
        control_ids,
        slot_env,
        clock_env,
    } = ctx;
    let body_arity = box_arity_typed(arena, body, cache)?;
    if is_const_zero(arena, clock) {
        let mut b = SigBuilder::new(arena);
        let zero = b.int(0);
        return Ok(vec![zero; body_arity.outputs]);
    }
    if is_const_one(arena, clock) {
        return propagate_in_slot_env(arena, body, tail, cache, control_ids, slot_env, clock_env);
    }

    let clock_env2 = make_clock_env(arena, clock_env, wrapper_node.as_tree_id(), inputs);
    let x1: Vec<_> = {
        let mut b = SigBuilder::new(arena);
        tail.iter().copied().map(|sig| b.temp_var(sig)).collect()
    };
    let x2: Vec<_> = {
        let mut b = SigBuilder::new(arena);
        x1.iter()
            .copied()
            .map(|sig| {
                let clocked = b.double_clocked(clock_env2, clock_env, sig);
                match kind {
                    ClockedWrapperKind::Ondemand | ClockedWrapperKind::Downsampling => clocked,
                    ClockedWrapperKind::Upsampling => b.zero_pad(clocked, clock),
                }
            })
            .collect()
    };
    let y0 = propagate_in_slot_env(arena, body, &x2, cache, control_ids, slot_env, clock_env2)?;

    let y1: Vec<_> = {
        let mut b = SigBuilder::new(arena);
        y0.iter()
            .copied()
            .map(|sig| {
                let clocked_sig = b.clocked(clock_env2, sig);
                b.perm_var(clocked_sig)
            })
            .collect()
    };
    let wrapper = {
        let mut b = SigBuilder::new(arena);
        let clocked_clock = b.clocked(clock_env2, clock);
        let mut wrapper_payload = Vec::with_capacity(y1.len() + 1);
        wrapper_payload.push(clocked_clock);
        wrapper_payload.extend(y1.iter().copied());
        match kind {
            ClockedWrapperKind::Ondemand => b.on_demand(&wrapper_payload),
            ClockedWrapperKind::Upsampling => b.upsampling(&wrapper_payload),
            ClockedWrapperKind::Downsampling => b.downsampling(&wrapper_payload),
        }
    };

    let mut b = SigBuilder::new(arena);
    Ok(y1.into_iter().map(|sig| b.seq(wrapper, sig)).collect())
}

fn is_const_zero(arena: &TreeArena, sig: SigId) -> bool {
    match match_sig(arena, sig) {
        SigMatch::Int(value) => value == 0,
        SigMatch::Real(value) => value == 0.0,
        _ => false,
    }
}

fn is_const_one(arena: &TreeArena, sig: SigId) -> bool {
    match match_sig(arena, sig) {
        SigMatch::Int(value) => value == 1,
        SigMatch::Real(value) => value == 1.0,
        _ => false,
    }
}

/// Validates that a primitive receives exactly the expected number of inputs.
fn expect_input_arity(
    node: TreeId,
    inputs: &[SigId],
    expected: usize,
) -> Result<(), PropagateError> {
    if inputs.len() == expected {
        Ok(())
    } else {
        Err(PropagateError::InputArityMismatch {
            node,
            expected,
            got: inputs.len(),
        })
    }
}

/// Lowers one unary primitive and returns a single output signal.
fn unary_prim(
    arena: &mut TreeArena,
    node: TreeId,
    inputs: &[SigId],
    f: impl FnOnce(&mut SigBuilder<'_>, SigId) -> SigId,
) -> Result<Vec<SigId>, PropagateError> {
    expect_input_arity(node, inputs, 1)?;
    let mut b = SigBuilder::new(arena);
    Ok(vec![f(&mut b, inputs[0])])
}

/// Lowers one binary primitive and returns a single output signal.
fn binary_prim(
    arena: &mut TreeArena,
    node: TreeId,
    inputs: &[SigId],
    f: impl FnOnce(&mut SigBuilder<'_>, SigId, SigId) -> SigId,
) -> Result<Vec<SigId>, PropagateError> {
    expect_input_arity(node, inputs, 2)?;
    let mut b = SigBuilder::new(arena);
    Ok(vec![f(&mut b, inputs[0], inputs[1])])
}

/// Lowers one ternary primitive and returns a single output signal.
fn ternary_prim(
    arena: &mut TreeArena,
    node: TreeId,
    inputs: &[SigId],
    f: impl FnOnce(&mut SigBuilder<'_>, SigId, SigId, SigId) -> SigId,
) -> Result<Vec<SigId>, PropagateError> {
    expect_input_arity(node, inputs, 3)?;
    let mut b = SigBuilder::new(arena);
    Ok(vec![f(&mut b, inputs[0], inputs[1], inputs[2])])
}

/// Lowers one quaternary primitive and returns a single output signal.
fn quaternary_prim(
    arena: &mut TreeArena,
    node: TreeId,
    inputs: &[SigId],
    f: impl FnOnce(&mut SigBuilder<'_>, SigId, SigId, SigId, SigId) -> SigId,
) -> Result<Vec<SigId>, PropagateError> {
    expect_input_arity(node, inputs, 4)?;
    let mut b = SigBuilder::new(arena);
    Ok(vec![f(&mut b, inputs[0], inputs[1], inputs[2], inputs[3])])
}

/// Lowers one quinary primitive and returns a single output signal.
fn quinary_prim(
    arena: &mut TreeArena,
    node: TreeId,
    inputs: &[SigId],
    f: impl FnOnce(&mut SigBuilder<'_>, SigId, SigId, SigId, SigId, SigId) -> SigId,
) -> Result<Vec<SigId>, PropagateError> {
    expect_input_arity(node, inputs, 5)?;
    let mut b = SigBuilder::new(arena);
    Ok(vec![f(
        &mut b, inputs[0], inputs[1], inputs[2], inputs[3], inputs[4],
    )])
}

/// Returns whether `split` wiring law is satisfied.
///
/// C++ parity rule:
/// - exact match, or
/// - right inputs is an integer multiple of left outputs.
fn split_compatible(left_outputs: usize, right_inputs: usize) -> bool {
    (left_outputs == right_inputs)
        || (left_outputs != 0 && right_inputs.is_multiple_of(left_outputs))
}

/// Returns whether `merge` wiring law is satisfied.
///
/// C++ parity rule:
/// - exact match, or
/// - left outputs is an integer multiple of right inputs.
fn merge_compatible(left_outputs: usize, right_inputs: usize) -> bool {
    (left_outputs == right_inputs)
        || (right_inputs != 0 && left_outputs.is_multiple_of(right_inputs))
}

/// Replicates input buses cyclically to feed `split` right-side arity.
fn split_signals(inputs: &[SigId], nbus: usize) -> Vec<SigId> {
    if nbus == 0 || inputs.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(nbus);
    for b in 0..nbus {
        out.push(inputs[b % inputs.len()]);
    }
    out
}

/// Mixes grouped buses by summing channels modulo `nbus` (merge semantics).
fn mix_signals(arena: &mut TreeArena, inputs: &[SigId], nbus: usize) -> Vec<SigId> {
    if nbus == 0 {
        return Vec::new();
    }

    let mut b = SigBuilder::new(arena);
    let mut out = Vec::with_capacity(nbus);

    for bus in 0..nbus {
        let mut acc = if bus < inputs.len() {
            inputs[bus]
        } else {
            b.int(0)
        };
        let mut idx = bus + nbus;
        while idx < inputs.len() {
            acc = b.add(acc, inputs[idx]);
            idx += nbus;
        }
        out.push(acc);
    }

    out
}

/// Returns list length for a `cons`/`nil` encoded list.
fn list_length(arena: &TreeArena, mut list: TreeId) -> Option<usize> {
    let mut len = 0usize;
    while !arena.is_nil(list) {
        let _ = arena.hd(list)?;
        list = arena.tl(list)?;
        len = len.checked_add(1)?;
    }
    Some(len)
}

/// Builds the adapted Rust clock-environment payload threaded through clocked wrappers.
///
/// C++ stores `(parent, slotenv, path, box, inputs...)` as a tree list. Rust
/// currently mirrors the same child ordering but leaves `slotenv` and `path`
/// empty because `crates/propagate` has not yet ported those lookup layers.
fn make_clock_env(
    arena: &mut TreeArena,
    parent: TreeId,
    box_node: TreeId,
    inputs: &[SigId],
) -> TreeId {
    let nil = arena.nil();
    let input_list = vec_to_list(arena, inputs);
    let tail3 = arena.cons(box_node, input_list);
    let tail2 = arena.cons(nil, tail3);
    let tail1 = arena.cons(nil, tail2);
    arena.cons(parent, tail1)
}

/// Flattens a route specification encoded as nested `par(...)` pairs into integer endpoints.
///
/// This mirrors the C++ `flattenRouteList(...)` helper used before `route`
/// propagation. The function only validates the already-built structural
/// payload; it does not normalize or evaluate the route expression.
fn flatten_route_ints(arena: &TreeArena, route_spec: TreeId) -> Result<Vec<i64>, PropagateError> {
    let mut out = Vec::new();
    flatten_route_ints_into(arena, route_spec, &mut out)?;
    Ok(out)
}

fn flatten_route_ints_into(
    arena: &TreeArena,
    node: TreeId,
    out: &mut Vec<i64>,
) -> Result<(), PropagateError> {
    match match_box(arena, node) {
        BoxMatch::Par(left, right) => {
            flatten_route_ints_into(arena, left, out)?;
            flatten_route_ints_into(arena, right, out)?;
            Ok(())
        }
        _ => {
            let Some(value) = tree_to_int(arena, node) else {
                return Err(PropagateError::UnsupportedBox {
                    node,
                    kind: "route-spec",
                });
            };
            out.push(value);
            Ok(())
        }
    }
}

/// Reads a non-negative integer node and converts it to `usize`.
fn usize_from_int_node(
    arena: &TreeArena,
    node: TreeId,
    field: &'static str,
) -> Result<usize, PropagateError> {
    let Some(value) = tree_to_int(arena, node) else {
        return Err(PropagateError::InvalidIntegerValue { node, field });
    };
    if value < 0 {
        return Err(PropagateError::NegativeIntegerValue { field, value });
    }
    usize::try_from(value).map_err(|_| PropagateError::InvalidIntegerValue { node, field })
}

/// Returns the C++ `ffarity(...)` for one wrapped foreign function descriptor.
fn ffunction_arity(arena: &TreeArena, ff: TreeId) -> Result<usize, PropagateError> {
    let BoxMatch::Ffunction(signature, _, _) = match_box(arena, ff) else {
        return Err(PropagateError::UnsupportedBox {
            node: ff,
            kind: "ffunction",
        });
    };
    let signature_len = list_length(arena, signature).ok_or(PropagateError::UnsupportedBox {
        node: signature,
        kind: "ffunction-signature",
    })?;
    signature_len
        .checked_sub(2)
        .ok_or(PropagateError::UnsupportedBox {
            node: signature,
            kind: "ffunction-signature",
        })
}

/// Fallible `usize -> i32` conversion used for stable signal-index nodes.
fn i32_from_usize(value: usize, field: &'static str) -> Result<i32, PropagateError> {
    i32::try_from(value).map_err(|_| PropagateError::IntegerTooLarge { field, value })
}

/// Seeds recursive feedback inputs with `delay1(proj(i, DEBRUIJNREF(1)))`.
fn make_mem_sig_proj_list(arena: &mut TreeArena, n: usize) -> Result<Vec<SigId>, PropagateError> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let idx = i32_from_usize(i, "rec projection seed index")?;
        let rg = debruijn_ref(arena, 1);
        let mut b = SigBuilder::new(arena);
        let proj = b.proj(idx, rg);
        out.push(b.delay1(proj));
    }
    Ok(out)
}

/// Lifts De Bruijn references of input signals by one recursion level.
///
/// Results are memoised across all input signals in this call.
fn lift_signals(arena: &mut TreeArena, inputs: &[SigId]) -> Vec<SigId> {
    let mut memo: AHashMap<(TreeId, i64), TreeId> = AHashMap::default();
    let mut out = Vec::with_capacity(inputs.len());
    for sig in inputs.iter().copied() {
        out.push(liftn(arena, sig, 1, &mut memo));
    }
    out
}

/// Builds one recursive signal group wrapper (`DEBRUIJN(body)`).
fn debruijn_rec(arena: &mut TreeArena, body: TreeId) -> TreeId {
    intern_tag(arena, DEBRUIJN_TAG, &[body])
}

/// Builds one De Bruijn reference node (`DEBRUIJNREF(level)`).
fn debruijn_ref(arena: &mut TreeArena, level: i64) -> TreeId {
    let lvl = arena.int(level);
    intern_tag(arena, DEBRUIJNREF_TAG, &[lvl])
}

/// Recursively lifts De Bruijn reference levels starting at `threshold`.
///
/// Results for each `(root, threshold)` pair are memoised in `memo` so that
/// shared sub-trees in the signal DAG are only traversed once.
fn liftn(
    arena: &mut TreeArena,
    root: TreeId,
    threshold: i64,
    memo: &mut AHashMap<(TreeId, i64), TreeId>,
) -> TreeId {
    if let Some(&cached) = memo.get(&(root, threshold)) {
        return cached;
    }
    let result = liftn_inner(arena, root, threshold, memo);
    memo.insert((root, threshold), result);
    result
}

fn liftn_inner(
    arena: &mut TreeArena,
    root: TreeId,
    threshold: i64,
    memo: &mut AHashMap<(TreeId, i64), TreeId>,
) -> TreeId {
    if let Some(level) = debruijn_ref_level(arena, root) {
        if level < threshold {
            return root;
        }
        return debruijn_ref(arena, level + 1);
    }

    if let Some(body) = debruijn_body(arena, root) {
        let lifted_body = liftn(arena, body, threshold + 1, memo);
        return debruijn_rec(arena, lifted_body);
    }

    let Some(node) = arena.node(root).cloned() else {
        return root;
    };
    if node.children.is_empty() {
        return root;
    }

    let original_children: Vec<TreeId> = node.children.as_slice().to_vec();
    let mut rebuilt = Vec::with_capacity(original_children.len());
    let mut changed = false;
    for child in original_children.iter().copied() {
        let lifted = liftn(arena, child, threshold, memo);
        if lifted != child {
            changed = true;
        }
        rebuilt.push(lifted);
    }
    if changed {
        arena.intern(node.kind, &rebuilt)
    } else {
        root
    }
}

/// Computes free-recursion aperture used to decide `sigProj` re-emission.
///
/// Results for each `root` are memoised in `memo` so that shared sub-trees in
/// the signal DAG are only traversed once.
fn aperture(arena: &TreeArena, root: TreeId, memo: &mut AHashMap<TreeId, i64>) -> i64 {
    if let Some(&cached) = memo.get(&root) {
        return cached;
    }
    let result = aperture_inner(arena, root, memo);
    memo.insert(root, result);
    result
}

fn aperture_inner(arena: &TreeArena, root: TreeId, memo: &mut AHashMap<TreeId, i64>) -> i64 {
    if let Some(level) = debruijn_ref_level(arena, root) {
        return level;
    }

    if let Some(body) = debruijn_body(arena, root) {
        return aperture(arena, body, memo) - 1;
    }

    let Some(children) = arena.children(root) else {
        return 0;
    };
    let children: Vec<TreeId> = children.to_vec();
    let mut max_aperture = 0;
    for child in children {
        max_aperture = max_aperture.max(aperture(arena, child, memo));
    }
    max_aperture
}

/// Returns De Bruijn level for a reference node, if `root` is `DEBRUIJNREF`.
fn debruijn_ref_level(arena: &TreeArena, root: TreeId) -> Option<i64> {
    let (tag, children) = tag_and_children(arena, root)?;
    if tag != DEBRUIJNREF_TAG {
        return None;
    }
    let [level_node] = children else {
        return None;
    };
    tree_to_int(arena, *level_node)
}

/// Returns recursive group body when `root` is a `DEBRUIJN` node.
fn debruijn_body(arena: &TreeArena, root: TreeId) -> Option<TreeId> {
    let (tag, children) = tag_and_children(arena, root)?;
    if tag != DEBRUIJN_TAG {
        return None;
    }
    let [body] = children else {
        return None;
    };
    Some(*body)
}

/// Helper to decode `(tag_name, children)` from one tagged node.
fn tag_and_children(arena: &TreeArena, root: TreeId) -> Option<(&str, &[TreeId])> {
    let node = arena.node(root)?;
    let NodeKind::Tag(tag_id) = &node.kind else {
        return None;
    };
    let tag = arena.tag_name(*tag_id)?;
    Some((tag, node.children.as_slice()))
}

/// Interns one tag node with children in the arena.
fn intern_tag(arena: &mut TreeArena, tag: &str, children: &[TreeId]) -> TreeId {
    let tag_id = arena.intern_tag(tag);
    arena.intern(NodeKind::Tag(tag_id), children)
}
