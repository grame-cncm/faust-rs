//! Post-eval flat-box boundary validation.
//!
//! `FlatBoxId` wraps a raw `TreeId` only after validating that the box belongs
//! to the first-order subset expected by propagation after `evalprocess` and
//! `a2sb`. This keeps evaluator-only syntax out of the propagation engine while
//! preserving `TreeArena` sharing.

use super::*;

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
    /// Returns the underlying [`TreeId`] for callers that need raw arena access.
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
pub(crate) enum FlatNodeKind {
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
    ForwardAD { body: FlatBoxId, seed: FlatBoxId },
    ReverseAD { body: FlatBoxId, seeds: FlatBoxId },
    Ondemand(FlatBoxId),
    Upsampling(FlatBoxId),
    Downsampling(FlatBoxId),
}

/// Validates that `root` belongs to the flat post-eval box subset and returns a typed handle.
///
/// This is a structural contract check only. It does not evaluate, simplify, or
/// normalize the tree. Callers should use it at the `eval/a2sb -> propagate`
/// boundary to guarantee that propagation never sees residual evaluator syntax.
///
/// Validation walks the reachable box DAG once with a `visited` set. This keeps
/// the adapted Rust `FlatBoxId` boundary linear on shared post-eval graphs
/// (common in large library combinators) instead of recursively re-validating
/// the same subtrees at every parent decode.
pub fn try_build_flat_box(arena: &TreeArena, root: BoxId) -> Result<FlatBoxId, FlatBoxBuildError> {
    let flat = FlatBoxId::from_tree_id(root);
    let mut visited = AHashSet::new();
    validate_flat_box_recursive(arena, flat, &mut visited)?;
    Ok(flat)
}

fn validate_flat_box_recursive(
    arena: &TreeArena,
    node: FlatBoxId,
    visited: &mut AHashSet<FlatBoxId>,
) -> Result<(), FlatBoxBuildError> {
    if !visited.insert(node) {
        return Ok(());
    }

    match flat_node_kind(arena, node)? {
        FlatNodeKind::ForwardAD { body, seed } => {
            validate_flat_box_recursive(arena, body, visited)?;
            validate_flat_box_recursive(arena, seed, visited)?;
        }
        FlatNodeKind::ReverseAD { body, seeds } => {
            validate_flat_box_recursive(arena, body, visited)?;
            validate_flat_box_recursive(arena, seeds, visited)?;
        }
        FlatNodeKind::Symbolic { body }
        | FlatNodeKind::Metadata { body }
        | FlatNodeKind::VGroup { body }
        | FlatNodeKind::HGroup { body }
        | FlatNodeKind::TGroup { body }
        | FlatNodeKind::Ondemand(body)
        | FlatNodeKind::Upsampling(body)
        | FlatNodeKind::Downsampling(body) => validate_flat_box_recursive(arena, body, visited)?,
        FlatNodeKind::Seq(left, right)
        | FlatNodeKind::Par(left, right)
        | FlatNodeKind::Split(left, right)
        | FlatNodeKind::Merge(left, right)
        | FlatNodeKind::Rec(left, right) => {
            validate_flat_box_recursive(arena, left, visited)?;
            validate_flat_box_recursive(arena, right, visited)?;
        }
        _ => {}
    }

    Ok(())
}

pub(crate) fn flat_node_kind(
    arena: &TreeArena,
    node: FlatBoxId,
) -> Result<FlatNodeKind, FlatBoxBuildError> {
    let node_id = node.as_tree_id();
    match match_box(arena, node_id) {
        BoxMatch::Int(_) => Ok(FlatNodeKind::Int),
        BoxMatch::Real(_) => Ok(FlatNodeKind::Real),
        BoxMatch::Wire => Ok(FlatNodeKind::Wire),
        BoxMatch::Cut => Ok(FlatNodeKind::Cut),
        BoxMatch::Slot(_) => Ok(FlatNodeKind::Slot),
        BoxMatch::Symbolic(_, body) => Ok(FlatNodeKind::Symbolic {
            body: FlatBoxId::from_tree_id(body),
        }),
        BoxMatch::Metadata(body, _) => Ok(FlatNodeKind::Metadata {
            body: FlatBoxId::from_tree_id(body),
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
        | BoxMatch::LRsh
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
        | BoxMatch::Exp10
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
            body: FlatBoxId::from_tree_id(body),
        }),
        BoxMatch::HGroup(_, body) => Ok(FlatNodeKind::HGroup {
            body: FlatBoxId::from_tree_id(body),
        }),
        BoxMatch::TGroup(_, body) => Ok(FlatNodeKind::TGroup {
            body: FlatBoxId::from_tree_id(body),
        }),
        BoxMatch::Seq(left, right) => Ok(FlatNodeKind::Seq(
            FlatBoxId::from_tree_id(left),
            FlatBoxId::from_tree_id(right),
        )),
        BoxMatch::Par(left, right) => Ok(FlatNodeKind::Par(
            FlatBoxId::from_tree_id(left),
            FlatBoxId::from_tree_id(right),
        )),
        BoxMatch::Split(left, right) => Ok(FlatNodeKind::Split(
            FlatBoxId::from_tree_id(left),
            FlatBoxId::from_tree_id(right),
        )),
        BoxMatch::Merge(left, right) => Ok(FlatNodeKind::Merge(
            FlatBoxId::from_tree_id(left),
            FlatBoxId::from_tree_id(right),
        )),
        BoxMatch::Rec(left, right) => Ok(FlatNodeKind::Rec(
            FlatBoxId::from_tree_id(left),
            FlatBoxId::from_tree_id(right),
        )),
        BoxMatch::Environment => Ok(FlatNodeKind::Environment),
        BoxMatch::Route(_, _, _) => Ok(FlatNodeKind::Route),
        BoxMatch::Inputs(_) => Ok(FlatNodeKind::Inputs),
        BoxMatch::Outputs(_) => Ok(FlatNodeKind::Outputs),
        BoxMatch::ForwardAD(body, seed) => Ok(FlatNodeKind::ForwardAD {
            body: FlatBoxId::from_tree_id(body),
            seed: FlatBoxId::from_tree_id(seed),
        }),
        BoxMatch::ReverseAD(body, seeds) => Ok(FlatNodeKind::ReverseAD {
            body: FlatBoxId::from_tree_id(body),
            seeds: FlatBoxId::from_tree_id(seeds),
        }),
        BoxMatch::Ondemand(body) => Ok(FlatNodeKind::Ondemand(FlatBoxId::from_tree_id(body))),
        BoxMatch::Upsampling(body) => Ok(FlatNodeKind::Upsampling(FlatBoxId::from_tree_id(body))),
        BoxMatch::Downsampling(body) => {
            Ok(FlatNodeKind::Downsampling(FlatBoxId::from_tree_id(body)))
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
        // C++ import-file nodes must be expanded away by the parser before
        // propagation. Reaching this arm means the structural import parity
        // contract was violated upstream.
        BoxMatch::ImportFile(_) => Err(flat_box_unexpected(node_id, "importfile")),
        BoxMatch::Case(_) => Err(flat_box_unexpected(node_id, "case")),
        BoxMatch::PatternMatcher(_) => Err(flat_box_unexpected(node_id, "patternmatcher")),
        BoxMatch::Closure(_) => Err(flat_box_unexpected(node_id, "closure")),
        BoxMatch::PatternVar(_) => Err(flat_box_unexpected(node_id, "patternvar")),
        BoxMatch::Abstr(_, _) => Err(flat_box_unexpected(node_id, "abstr")),
        BoxMatch::Modulation(_, _) => Err(flat_box_unexpected(node_id, "modulation")),
    }
}

/// Returns `true` when `box_tree` is or contains a `ForwardAD` node anywhere
/// in its composition tree.  Used by the Rec propagation handler to decide
/// whether to suppress FAD expansion during branch propagation and defer it
/// to after the recursive group is fully built.
pub(crate) fn contains_forward_ad(
    arena: &TreeArena,
    box_tree: FlatBoxId,
    cache: &mut ArityCache,
) -> Result<bool, FlatBoxBuildError> {
    if let Some(&cached) = cache.forward_ad.get(&box_tree) {
        return Ok(cached);
    }
    let found = match flat_node_kind(arena, box_tree)? {
        FlatNodeKind::ForwardAD { .. } => true,
        FlatNodeKind::Rec(left, right)
        | FlatNodeKind::Seq(left, right)
        | FlatNodeKind::Par(left, right)
        | FlatNodeKind::Split(left, right)
        | FlatNodeKind::Merge(left, right) => {
            contains_forward_ad(arena, left, cache)? || contains_forward_ad(arena, right, cache)?
        }
        FlatNodeKind::ReverseAD { body, seeds } => {
            contains_forward_ad(arena, body, cache)? || contains_forward_ad(arena, seeds, cache)?
        }
        FlatNodeKind::Symbolic { body }
        | FlatNodeKind::Metadata { body }
        | FlatNodeKind::VGroup { body }
        | FlatNodeKind::HGroup { body }
        | FlatNodeKind::TGroup { body }
        | FlatNodeKind::Ondemand(body)
        | FlatNodeKind::Upsampling(body)
        | FlatNodeKind::Downsampling(body) => contains_forward_ad(arena, body, cache)?,
        _ => false,
    };
    cache.forward_ad.insert(box_tree, found);
    Ok(found)
}

/// Counts the number of [`FlatNodeKind::ForwardAD`] nodes reachable in a flat
/// box tree. Used by `box_arity_typed` to predict the tangent expansion in
/// recursive compositions.
pub(crate) fn count_fad_nodes(
    arena: &TreeArena,
    box_tree: FlatBoxId,
    visited: &mut AHashSet<FlatBoxId>,
) -> Result<usize, PropagateError> {
    if !visited.insert(box_tree) {
        return Ok(0);
    }
    match flat_node_kind(arena, box_tree)? {
        FlatNodeKind::ForwardAD { .. } => Ok(1),
        FlatNodeKind::Rec(left, right)
        | FlatNodeKind::Seq(left, right)
        | FlatNodeKind::Par(left, right)
        | FlatNodeKind::Split(left, right)
        | FlatNodeKind::Merge(left, right) => {
            let l = count_fad_nodes(arena, left, visited)?;
            let r = count_fad_nodes(arena, right, visited)?;
            Ok(l + r)
        }
        FlatNodeKind::ReverseAD { body, seeds } => {
            let b = count_fad_nodes(arena, body, visited)?;
            let s = count_fad_nodes(arena, seeds, visited)?;
            Ok(b + s)
        }
        FlatNodeKind::Symbolic { body }
        | FlatNodeKind::Metadata { body }
        | FlatNodeKind::VGroup { body }
        | FlatNodeKind::HGroup { body }
        | FlatNodeKind::TGroup { body }
        | FlatNodeKind::Ondemand(body)
        | FlatNodeKind::Upsampling(body)
        | FlatNodeKind::Downsampling(body) => count_fad_nodes(arena, body, visited),
        _ => Ok(0),
    }
}

/// `ExpandAfterRec` preserves the historical Rust behavior where `ForwardAD`
/// stays arity-transparent during internal recursive wiring and the tangent
/// bundle is emitted only after the recursive group has been built.
///
/// `AugmentedState` is selected when a recursive branch needs to consume the
/// expanded `[primal, tangent]` outputs locally before the `Rec` boundary. In
/// that mode the recursive group itself is built on the real expanded AD arity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecFadMode {
    None,
    ExpandAfterRec,
    AugmentedState,
}

/// Returns the recursive FAD handling mode required for one `boxRec(...)`.
///
/// The current heuristic is intentionally structural:
/// - no reachable `ForwardAD` → [`RecFadMode::None`],
/// - reachable `ForwardAD` only through transparent wrappers
///   (`vgroup`/`hgroup`/`tgroup`/`metadata`/`symbolic`) until the `Rec` branch
///   root → [`RecFadMode::ExpandAfterRec`],
/// - any composition/algebra node consuming a `ForwardAD` subtree locally
///   before the `Rec` boundary → [`RecFadMode::AugmentedState`].
pub(crate) fn rec_fad_mode(
    arena: &TreeArena,
    left: FlatBoxId,
    right: FlatBoxId,
    cache: &mut ArityCache,
) -> Result<RecFadMode, PropagateError> {
    let left_has = contains_forward_ad(arena, left, cache)?;
    let right_has = contains_forward_ad(arena, right, cache)?;
    if !left_has && !right_has {
        return Ok(RecFadMode::None);
    }
    let left_local = subtree_consumes_fad_outputs_locally(arena, left, false, cache)?;
    let right_local = subtree_consumes_fad_outputs_locally(arena, right, false, cache)?;
    if left_local || right_local {
        Ok(RecFadMode::AugmentedState)
    } else {
        Ok(RecFadMode::ExpandAfterRec)
    }
}

/// Returns `true` when a `ForwardAD` subtree is consumed by a non-transparent
/// operator before reaching the recursive branch root.
fn subtree_consumes_fad_outputs_locally(
    arena: &TreeArena,
    box_tree: FlatBoxId,
    consumed_by_parent: bool,
    cache: &mut ArityCache,
) -> Result<bool, PropagateError> {
    let key = (box_tree, consumed_by_parent);
    if let Some(&cached) = cache.fad_consumed_locally.get(&key) {
        return Ok(cached);
    }
    let found = match flat_node_kind(arena, box_tree)? {
        FlatNodeKind::ForwardAD { .. } => consumed_by_parent,
        FlatNodeKind::Symbolic { body }
        | FlatNodeKind::Metadata { body }
        | FlatNodeKind::VGroup { body }
        | FlatNodeKind::HGroup { body }
        | FlatNodeKind::TGroup { body } => {
            subtree_consumes_fad_outputs_locally(arena, body, consumed_by_parent, cache)?
        }
        FlatNodeKind::Rec(left, right)
        | FlatNodeKind::Seq(left, right)
        | FlatNodeKind::Par(left, right)
        | FlatNodeKind::Split(left, right)
        | FlatNodeKind::Merge(left, right) => {
            subtree_consumes_fad_outputs_locally(arena, left, true, cache)?
                || subtree_consumes_fad_outputs_locally(arena, right, true, cache)?
        }
        FlatNodeKind::ReverseAD { body, seeds } => {
            subtree_consumes_fad_outputs_locally(arena, body, true, cache)?
                || subtree_consumes_fad_outputs_locally(arena, seeds, true, cache)?
        }
        FlatNodeKind::Ondemand(body)
        | FlatNodeKind::Upsampling(body)
        | FlatNodeKind::Downsampling(body) => {
            subtree_consumes_fad_outputs_locally(arena, body, true, cache)?
        }
        _ => false,
    };
    cache.fad_consumed_locally.insert(key, found);
    Ok(found)
}

fn flat_box_unexpected(node: TreeId, kind: &'static str) -> FlatBoxBuildError {
    FlatBoxBuildError::UnexpectedPostEvalBox { node, kind }
}
