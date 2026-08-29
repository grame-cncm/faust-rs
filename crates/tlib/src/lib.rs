#![forbid(unsafe_code)]

//! `tlib` foundation crate for Faust tree-like compiler data.
//!
//! # Source provenance (C++)
//! - `compiler/tlib/tree.hh`, `compiler/tlib/tree.cpp`
//! - `compiler/tlib/list.hh`, `compiler/tlib/list.cpp`
//! - `compiler/tlib/property.hh`
//! - `compiler/tlib/node.hh`, `compiler/tlib/symbol.hh`
//!
//! # Parity invariants
//! - Structural hash-consing: identical `(NodeKind, children)` must map to the same `TreeId`.
//! - `nil/cons` list behavior follows Faust C++ conventions (`cons`, `hd`, `tl`, `isNil`, `isList`).
//! - Properties are node-keyed and support fast keyed access for parser/evaluator hot paths.
//! - [`vec_to_list`] / [`list_to_vec`] provide the shared `cons`/`nil` list
//!   adapters reused by later compiler stages.
//!
//! # Recursive trees (Phase 5)
//! The crate now exposes explicit recursive-tree helpers for de Bruijn and symbolic forms:
//! - De Bruijn builders/matchers: [`de_bruijn_rec`], [`de_bruijn_ref`],
//!   [`match_de_bruijn_rec`], [`match_de_bruijn_ref`]
//! - Symbolic builders/matchers: [`sym_rec`], [`sym_ref`], [`match_sym_rec`], [`match_sym_ref`]
//! - Conversion and analysis helpers: [`de_bruijn_to_sym`], [`de_bruijn_aperture`],
//!   [`is_de_bruijn_closed`], [`check_de_bruijn_coherence`], [`lift_de_bruijn`],
//!   [`lift_de_bruijn_n`]
//! - Validation helpers: [`validate_faust_list`], [`validate_closed_de_bruijn_tree`],
//!   [`validate_symbolic_recursion_tree`]
//!
//! Mapping note:
//! - `deBruijn2Sym(Tree)` from C++ is ported as [`de_bruijn_to_sym`] with explicit
//!   [`RecursionError`] returns instead of process-global assertions.
//!
//! Pipeline contract note:
//! - the fast-lane now clones the whole signal forest into a private staging arena
//!   and applies [`de_bruijn_to_sym`] before FIR lowering.
//! - normalization-level symbolic conversion remains opt-in at other call-site
//!   boundaries that expose signal trees directly.

mod arena;
mod property;
mod recursion;

pub use arena::{ChildList, NodeKind, TreeArena, TreeId, TreeNode};
pub use property::{PropertyKey, PropertyStore};
pub use recursion::{
    DEBRUIJNREC_TAG, DEBRUIJNREF_TAG, RecursionError, SYMREC_TAG, SYMREF_TAG,
    SymbolicRecursionValidationError, check_de_bruijn_coherence, de_bruijn_aperture,
    de_bruijn_aperture_with_memo, de_bruijn_rec, de_bruijn_ref, de_bruijn_to_sym,
    de_bruijn_to_sym_many, is_de_bruijn_closed, lift_de_bruijn, lift_de_bruijn_n,
    match_de_bruijn_rec, match_de_bruijn_ref, match_sym_rec, match_sym_ref, sym_rec, sym_ref,
    validate_closed_de_bruijn_tree, validate_symbolic_recursion_tree,
};

/// Stable crate identifier used by workspace tooling and diagnostics.
pub const CRATE_NAME: &str = "tlib";

/// Stable crate identifier used in workspace-level tooling and diagnostics.
#[must_use]
pub fn crate_id() -> &'static str {
    CRATE_NAME
}

/// Extracts the symbol name from one tree node.
///
/// Returns `None` when the node is not a `Symbol`.
/// This is the Rust equivalent of the C++ `tree2str` function.
#[must_use]
pub fn tree_to_str(arena: &TreeArena, id: TreeId) -> Option<&str> {
    match arena.kind(id) {
        Some(NodeKind::Symbol(s)) => Some(s),
        _ => None,
    }
}

/// Extracts an integer atom value from one tree node.
///
/// Returns `None` when the node is not an `Int`.
#[must_use]
pub fn tree_to_int(arena: &TreeArena, id: TreeId) -> Option<i64> {
    match arena.kind(id) {
        Some(NodeKind::Int(v)) => Some(*v),
        _ => None,
    }
}

/// Extracts a floating-point atom value from one tree node.
///
/// Returns `None` when the node is not a `FloatBits`.
#[must_use]
pub fn tree_to_double(arena: &TreeArena, id: TreeId) -> Option<f64> {
    match arena.kind(id) {
        Some(NodeKind::FloatBits(bits)) => Some(f64::from_bits(*bits)),
        _ => None,
    }
}

/// Typed error returned by [`validate_faust_list`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListValidationError {
    /// The provided `TreeId` does not exist in the arena.
    InvalidNode { node: TreeId },
    /// The tree is not a canonical `cons`/`nil` Faust list.
    MalformedList { node: TreeId },
}

impl std::fmt::Display for ListValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidNode { node } => write!(f, "invalid node id {}", node.as_u32()),
            Self::MalformedList { node } => {
                write!(f, "malformed Faust list at node {}", node.as_u32())
            }
        }
    }
}

impl std::error::Error for ListValidationError {}

/// Builds a `cons`/`nil` list from one slice while preserving iteration order.
///
/// This mirrors the repeated list-construction helpers used throughout the C++
/// compiler on top of `cons`/`nil`.
#[must_use]
pub fn vec_to_list(arena: &mut TreeArena, values: &[TreeId]) -> TreeId {
    let mut list = arena.nil();
    for value in values.iter().rev().copied() {
        list = arena.cons(value, list);
    }
    list
}

/// Converts a `cons`/`nil` list into a vector preserving traversal order.
///
/// Returns `None` when the input is not a well-formed Faust list encoded as a
/// chain of `cons(head, tail)` cells terminated by `nil`.
///
/// This is intentionally strict: malformed list-shaped trees are rejected
/// rather than partially traversed, which keeps callers from silently
/// accepting non-canonical encodings.
#[must_use]
pub fn list_to_vec(arena: &TreeArena, mut list: TreeId) -> Option<Vec<TreeId>> {
    let mut out = Vec::new();
    while !arena.is_nil(list) {
        out.push(arena.hd(list)?);
        list = arena.tl(list)?;
    }
    Some(out)
}

/// Validates that `root` is a canonical `cons`/`nil` Faust list and returns its
/// length.
///
/// This is the executable counterpart of the list-shape contract already
/// relied upon by `vec_to_list`, `list_to_vec`, recursive-tree helpers, and
/// later compiler phases that store payload sequences in `TreeArena`.
pub fn validate_faust_list(
    arena: &TreeArena,
    mut root: TreeId,
) -> Result<usize, ListValidationError> {
    if arena.node(root).is_none() {
        return Err(ListValidationError::InvalidNode { node: root });
    }
    let mut len = 0usize;
    while !arena.is_nil(root) {
        if !arena.is_list(root) {
            return Err(ListValidationError::MalformedList { node: root });
        }
        let Some(_head) = arena.hd(root) else {
            return Err(ListValidationError::MalformedList { node: root });
        };
        let Some(tail) = arena.tl(root) else {
            return Err(ListValidationError::MalformedList { node: root });
        };
        len += 1;
        root = tail;
    }
    Ok(len)
}
