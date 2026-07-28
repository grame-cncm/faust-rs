//! Recursive-tree helpers (`DEBRUIJNREC`/`DEBRUIJNREF` and symbolic form).
//!
//! # Source provenance (C++)
//! - `compiler/tlib/recursive-tree.cpp` (`deBruijn2Sym`, `substitute`, `liftn`)
//! - `compiler/tlib/tree.hh` (recursive-tree API surface)
//!
//! # Public API mapping status
//! - `deBruijn2Sym(Tree)` -> [`de_bruijn_to_sym`] (`adapted`: returns `Result`)
//! - `lift/liftn(Tree, threshold)` -> [`lift_de_bruijn`] / [`lift_de_bruijn_n`] (`adapted`)
//! - symbolic recursion shape -> explicit tags [`SYMREC_TAG`]/[`SYMREF_TAG`] (`adapted`)
//!
//! # Parity invariants
//! - De Bruijn binder is `DEBRUIJNREC(body)`, reference is `DEBRUIJNREF(level)`.
//! - Symbolic recursion is explicit and deterministic:
//!   - `SYMREC(var, body)`
//!   - `SYMREF(var)`
//! - Conversion keeps structural sharing through memoized rebuilds.

use std::error::Error;
use std::fmt;

use ahash::AHashMap;

use crate::{NodeKind, TreeArena, TreeId, tree_to_int};

/// Tag for one canonical de Bruijn recursive group binder: `DEBRUIJNREC(body)`.
pub const DEBRUIJNREC_TAG: &str = "DEBRUIJNREC";
/// Tag for one de Bruijn reference payload: `DEBRUIJNREF(level)`.
pub const DEBRUIJNREF_TAG: &str = "DEBRUIJNREF";
/// Tag for one symbolic recursive binder: `SYMREC(var, body)`.
pub const SYMREC_TAG: &str = "SYMREC";
/// Tag for one symbolic recursive reference: `SYMREF(var)`.
pub const SYMREF_TAG: &str = "SYMREF";

/// Errors returned by recursive-tree conversion utilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecursionError {
    /// The provided root contains free de Bruijn references (`aperture > 0`).
    OpenDeBruijnTree { aperture: i64 },
    /// A `DEBRUIJNREF(level)` was found at a nesting depth where no enclosing
    /// `DEBRUIJNREC` binder can own it.
    IncoherentDeBruijnReference {
        node: TreeId,
        /// The bad one-based De Bruijn level stored in the `DEBRUIJNREF` node.
        level: i64,
        /// Number of enclosing `DEBRUIJNREC` binders at the point of discovery.
        depth: i64,
    },
    /// A de Bruijn reference remained during conversion, which indicates an unbound reference.
    UnboundDeBruijnReference { node: TreeId, level: i64 },
    /// One `DEBRUIJNREF` node did not have the expected `int` payload.
    MalformedDeBruijnReference { node: TreeId },
    /// One requested `TreeId` did not exist in the arena.
    InvalidNode { node: TreeId },
}

impl fmt::Display for RecursionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenDeBruijnTree { aperture } => {
                write!(f, "tree is open in de Bruijn form (aperture={aperture})")
            }
            Self::IncoherentDeBruijnReference { node, level, depth } => {
                write!(
                    f,
                    "De Bruijn coherence error: DEBRUIJNREF(level={level}) at depth={depth} - reference escapes its enclosing binders (node {})",
                    node.as_u32()
                )
            }
            Self::UnboundDeBruijnReference { node, level } => {
                write!(
                    f,
                    "unbound DEBRUIJNREF(level={level}) at node {} during conversion",
                    node.as_u32()
                )
            }
            Self::MalformedDeBruijnReference { node } => {
                write!(
                    f,
                    "malformed DEBRUIJNREF payload at node {} (expected int level)",
                    node.as_u32()
                )
            }
            Self::InvalidNode { node } => write!(f, "invalid node id {}", node.as_u32()),
        }
    }
}

impl Error for RecursionError {}

/// Structural validation errors for symbolic recursion trees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolicRecursionValidationError {
    /// The provided root or one traversed child does not exist in the arena.
    InvalidNode { node: TreeId },
    /// A list-shaped payload in a symbolic recursion group is malformed.
    MalformedList { node: TreeId },
    /// One `SYMREC(var, body_list)` group has an empty body list.
    EmptyGroup { node: TreeId },
    /// One symbolic reference is not bound by any enclosing symbolic group.
    UnboundReference { node: TreeId, var: TreeId },
}

impl fmt::Display for SymbolicRecursionValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNode { node } => write!(f, "invalid node id {}", node.as_u32()),
            Self::MalformedList { node } => write!(
                f,
                "malformed symbolic recursion body list at node {}",
                node.as_u32()
            ),
            Self::EmptyGroup { node } => write!(
                f,
                "symbolic recursion group {} has empty body list",
                node.as_u32()
            ),
            Self::UnboundReference { node, var } => write!(
                f,
                "symbolic recursion reference {} targets unbound variable {}",
                node.as_u32(),
                var.as_u32()
            ),
        }
    }
}

impl Error for SymbolicRecursionValidationError {}

/// Builds `DEBRUIJNREC(body)`.
#[must_use]
pub fn de_bruijn_rec(arena: &mut TreeArena, body: TreeId) -> TreeId {
    intern_tag(arena, DEBRUIJNREC_TAG, &[body])
}

/// Builds `DEBRUIJNREF(level)`.
#[must_use]
pub fn de_bruijn_ref(arena: &mut TreeArena, level: i64) -> TreeId {
    let level_id = arena.int(level);
    intern_tag(arena, DEBRUIJNREF_TAG, &[level_id])
}

/// Builds `SYMREC(var, body)` using the explicit symbolic shape.
#[must_use]
pub fn sym_rec(arena: &mut TreeArena, var: TreeId, body: TreeId) -> TreeId {
    intern_tag(arena, SYMREC_TAG, &[var, body])
}

/// Builds `SYMREF(var)` using the explicit symbolic shape.
#[must_use]
pub fn sym_ref(arena: &mut TreeArena, var: TreeId) -> TreeId {
    intern_tag(arena, SYMREF_TAG, &[var])
}

/// Matches `DEBRUIJNREC(body)`.
#[must_use]
pub fn match_de_bruijn_rec(arena: &TreeArena, id: TreeId) -> Option<TreeId> {
    let children = tag_children(arena, id, DEBRUIJNREC_TAG)?;
    match children {
        [body] => Some(*body),
        _ => None,
    }
}

/// Matches `DEBRUIJNREF(level)` and decodes `level`.
///
/// Returns `None` when node shape is not de Bruijn reference or payload is not `Int`.
#[must_use]
pub fn match_de_bruijn_ref(arena: &TreeArena, id: TreeId) -> Option<i64> {
    let children = tag_children(arena, id, DEBRUIJNREF_TAG)?;
    match children {
        [lvl] => tree_to_int(arena, *lvl),
        _ => None,
    }
}

/// Matches symbolic binder `SYMREC(var, body)`.
#[must_use]
pub fn match_sym_rec(arena: &TreeArena, id: TreeId) -> Option<(TreeId, TreeId)> {
    let children = tag_children(arena, id, SYMREC_TAG)?;
    match children {
        [var, body] => Some((*var, *body)),
        _ => None,
    }
}

/// Matches symbolic reference `SYMREF(var)`.
///
/// Compatibility note: one-child legacy `SYMREC(var)` is also accepted as symbolic ref.
#[must_use]
pub fn match_sym_ref(arena: &TreeArena, id: TreeId) -> Option<TreeId> {
    if let Some(children) = tag_children(arena, id, SYMREF_TAG)
        && let [var] = children
    {
        return Some(*var);
    }
    let children = tag_children(arena, id, SYMREC_TAG)?;
    match children {
        [var] => Some(*var),
        _ => None,
    }
}

/// Computes de Bruijn aperture (`<= 0` means closed).
///
/// Semantics mirror C++ `calcTreeAperture`:
/// - `DEBRUIJNREF(level)` -> `level`
/// - `DEBRUIJNREC(body)` -> `aperture(body) - 1`
/// - any other node -> `max(aperture(children))`
#[must_use]
pub fn de_bruijn_aperture(arena: &TreeArena, root: TreeId) -> i64 {
    let mut memo = AHashMap::new();
    de_bruijn_aperture_with_memo(arena, root, &mut memo)
}

/// Like [`de_bruijn_aperture`], but accepts an external memo cache.
///
/// This allows callers that compute aperture many times across a traversal
/// (e.g. `propagate`) to amortize the cost by sharing a single cache.
#[must_use]
pub fn de_bruijn_aperture_with_memo(
    arena: &TreeArena,
    root: TreeId,
    memo: &mut AHashMap<TreeId, i64>,
) -> i64 {
    aperture(arena, root, memo)
}

/// Returns `true` when `root` has no free de Bruijn references.
#[must_use]
pub fn is_de_bruijn_closed(arena: &TreeArena, root: TreeId) -> bool {
    de_bruijn_aperture(arena, root) <= 0
}

/// Verifies local De Bruijn binder coherence for the tree rooted at `root`.
///
/// Faust's C++ recursive-tree model, and this Rust port, use one-based De
/// Bruijn levels: `DEBRUIJNREF(1)` references the nearest enclosing
/// `DEBRUIJNREC`, `DEBRUIJNREF(2)` references the next outer group, and so on.
/// At every reference site, this function verifies `1 <= level <= depth`, where
/// `depth` is the number of enclosing `DEBRUIJNREC` binders on the current
/// traversal path.
///
/// Returns `Ok(())` if every reachable reference is locally bound. Returns
/// [`RecursionError::IncoherentDeBruijnReference`] on the first escaping or
/// zero-level reference, [`RecursionError::MalformedDeBruijnReference`] for a
/// malformed reference payload, and [`RecursionError::InvalidNode`] if traversal
/// reaches a missing node.
///
/// This is stricter than [`is_de_bruijn_closed`], which only checks the
/// root-level aperture. A tree can have a non-positive aperture while still
/// containing an invalid local level such as `DEBRUIJNREC(DEBRUIJNREF(0))`.
/// The memoization key is `(TreeId, depth)` because the same hash-consed node can
/// appear under different numbers of enclosing binders.
pub fn check_de_bruijn_coherence(arena: &TreeArena, root: TreeId) -> Result<(), RecursionError> {
    let mut memo = AHashMap::new();
    check_coherence_at_depth(arena, root, 0, &mut memo)
}

/// Lifts free de Bruijn references by one level (`liftn(..., 1)`).
#[must_use]
pub fn lift_de_bruijn(arena: &mut TreeArena, root: TreeId) -> TreeId {
    lift_de_bruijn_n(arena, root, 1)
}

/// Lifts free de Bruijn references with one threshold level (`liftn` parity).
///
/// References with `level < threshold` are considered bound and unchanged.
#[must_use]
pub fn lift_de_bruijn_n(arena: &mut TreeArena, root: TreeId, threshold: i64) -> TreeId {
    let mut lifter = Lifter::new(arena);
    lifter.lift(root, threshold)
}

/// Converts one closed de Bruijn recursion tree into symbolic recursion form.
///
/// Returns `Err` when the input tree is open (`aperture > 0`), malformed, or contains
/// unbound references during conversion.
pub fn de_bruijn_to_sym(arena: &mut TreeArena, root: TreeId) -> Result<TreeId, RecursionError> {
    let aperture = de_bruijn_aperture(arena, root);
    if aperture > 0 {
        return Err(RecursionError::OpenDeBruijnTree { aperture });
    }
    let mut converter = Converter::new(arena);
    converter.convert(root)
}

/// Converts several closed de Bruijn recursion trees through one shared
/// converter, guaranteeing that shared sub-terms across the roots map to the
/// same symbolic recursion node (and therefore the same `TreeId`).
///
/// Calling [`de_bruijn_to_sym`] once per root produces independent fresh
/// variable sequences: the first call consumes `W0`, the second observes `W0`
/// as already interned and picks `W1`, and so on. When two roots share a
/// de Bruijn recursion sub-term, that drift forks them into two distinct
/// symbolic recursions. Downstream consumers that compare by `TreeId`
/// (e.g. seed matching in forward-mode AD) then silently miss the shared
/// sub-term. This helper routes every root through one internal converter so the
/// memo and `next_var_index` are shared.
pub fn de_bruijn_to_sym_many(
    arena: &mut TreeArena,
    roots: &[TreeId],
) -> Result<Vec<TreeId>, RecursionError> {
    for &root in roots {
        let aperture = de_bruijn_aperture(arena, root);
        if aperture > 0 {
            return Err(RecursionError::OpenDeBruijnTree { aperture });
        }
    }
    let mut converter = Converter::new(arena);
    let mut out = Vec::with_capacity(roots.len());
    for &root in roots {
        out.push(converter.convert(root)?);
    }
    Ok(out)
}

/// Validates that the reachable de Bruijn recursion tree rooted at `root` is
/// closed and structurally acceptable for conversion.
///
/// The validation is side-effect free for the caller: the subtree is cloned into
/// a private arena and passed through [`de_bruijn_to_sym`], so malformed
/// references and open terms report the same typed errors as the actual
/// conversion boundary without mutating the source arena.
pub fn validate_closed_de_bruijn_tree(
    arena: &TreeArena,
    root: TreeId,
) -> Result<(), RecursionError> {
    if arena.node(root).is_none() {
        return Err(RecursionError::InvalidNode { node: root });
    }
    let mut staging = TreeArena::new();
    let cloned = staging.clone_subtree_from(arena, root);
    de_bruijn_to_sym(&mut staging, cloned).map(|_| ())
}

/// Validates that the reachable symbolic recursion tree rooted at `root` is
/// structurally well formed.
///
/// This checks only the symbolic recursion contract:
/// - body payloads are canonical `cons`/`nil` lists,
/// - symbolic groups are non-empty,
/// - `SYMREF(var)` targets a currently bound symbolic variable.
pub fn validate_symbolic_recursion_tree(
    arena: &TreeArena,
    root: TreeId,
) -> Result<(), SymbolicRecursionValidationError> {
    let mut bound = AHashMap::<TreeId, usize>::new();
    validate_symbolic_subtree(arena, root, &mut bound)
}

fn validate_symbolic_subtree(
    arena: &TreeArena,
    node: TreeId,
    bound: &mut AHashMap<TreeId, usize>,
) -> Result<(), SymbolicRecursionValidationError> {
    if arena.node(node).is_none() {
        return Err(SymbolicRecursionValidationError::InvalidNode { node });
    }
    if arena.is_nil(node) {
        return Ok(());
    }
    if arena.is_list(node) {
        let Some(head) = arena.hd(node) else {
            return Err(SymbolicRecursionValidationError::MalformedList { node });
        };
        let Some(tail) = arena.tl(node) else {
            return Err(SymbolicRecursionValidationError::MalformedList { node });
        };
        validate_symbolic_subtree(arena, head, bound)?;
        validate_symbolic_subtree(arena, tail, bound)?;
        return Ok(());
    }
    if let Some((var, body_list)) = match_sym_rec(arena, node) {
        let mut cursor = body_list;
        let mut arity = 0usize;
        while !arena.is_nil(cursor) {
            let Some(body) = arena.hd(cursor) else {
                return Err(SymbolicRecursionValidationError::MalformedList { node: cursor });
            };
            let Some(tail) = arena.tl(cursor) else {
                return Err(SymbolicRecursionValidationError::MalformedList { node: cursor });
            };
            arity += 1;
            *bound.entry(var).or_insert(0) += 1;
            validate_symbolic_subtree(arena, body, bound)?;
            let count = bound
                .get_mut(&var)
                .expect("symbolic recursion binder count exists while descending body");
            *count -= 1;
            if *count == 0 {
                bound.remove(&var);
            }
            cursor = tail;
        }
        if arity == 0 {
            return Err(SymbolicRecursionValidationError::EmptyGroup { node });
        }
        return Ok(());
    }
    if let Some(var) = match_sym_ref(arena, node) {
        if bound.get(&var).copied().unwrap_or(0) == 0 {
            return Err(SymbolicRecursionValidationError::UnboundReference { node, var });
        }
        return Ok(());
    }
    let current = arena
        .node(node)
        .expect("validated node id should remain present during traversal");
    for child in current.children.as_slice() {
        validate_symbolic_subtree(arena, *child, bound)?;
    }
    Ok(())
}

/// Stateful de Bruijn-to-symbolic converter.
///
/// This helper mirrors the staged C++ algorithm:
/// 1. allocate one fresh symbolic variable per encountered `DEBRUIJNREC(...)`
///    binder,
/// 2. substitute `DEBRUIJNREF(1)` in the binder body with `SYMREF(var)`,
/// 3. recursively rebuild the result while preserving sharing via memoization.
///
/// Separate memos are kept for:
/// - full-node conversion,
/// - substitution at `(node, level, replacement)`,
/// - aperture queries used to prune substitution work.
struct Converter<'a> {
    arena: &'a mut TreeArena,
    next_var_index: u64,
    convert_memo: AHashMap<TreeId, TreeId>,
    substitute_memo: AHashMap<(TreeId, i64, TreeId), TreeId>,
    aperture_memo: AHashMap<TreeId, i64>,
}

impl<'a> Converter<'a> {
    /// Creates one converter scoped to a single output arena/root conversion.
    fn new(arena: &'a mut TreeArena) -> Self {
        Self {
            arena,
            next_var_index: 0,
            convert_memo: AHashMap::new(),
            substitute_memo: AHashMap::new(),
            aperture_memo: AHashMap::new(),
        }
    }

    /// Allocates one deterministic fresh symbolic variable name (`W0`, `W1`, ...).
    ///
    /// The exact prefix is an implementation detail, but the sequence is stable
    /// for a given traversal order, which keeps converted trees deterministic.
    ///
    /// C++ parity note: this mirrors `unique("W")`, not a plain symbol lookup.
    /// The arena may already contain user or intermediate symbols named `W0`,
    /// `W1`, ... so this helper must skip pre-existing names and only return a
    /// symbol node that was freshly interned for the current conversion.
    fn fresh_var(&mut self) -> TreeId {
        loop {
            let name = format!("W{}", self.next_var_index);
            self.next_var_index += 1;
            let before = self.arena.len();
            let var = self.arena.symbol(name);
            if self.arena.len() > before {
                return var;
            }
        }
    }

    /// Converts one node from de Bruijn form to symbolic form.
    ///
    /// Non-recursive nodes are rebuilt recursively with converted children.
    /// `DEBRUIJNREC(...)` binders trigger the fresh-variable/substitute/rebuild
    /// sequence described on [`Converter`].
    fn convert(&mut self, id: TreeId) -> Result<TreeId, RecursionError> {
        if let Some(mapped) = self.convert_memo.get(&id) {
            return Ok(*mapped);
        }

        let Some(node) = self.arena.node(id).cloned() else {
            return Err(RecursionError::InvalidNode { node: id });
        };

        if let Some(body) = match_de_bruijn_rec(self.arena, id) {
            let var = self.fresh_var();
            let replacement = sym_ref(self.arena, var);
            let substituted = self.substitute(body, 1, replacement)?;
            let converted_body = self.convert(substituted)?;
            let out = sym_rec(self.arena, var, converted_body);
            self.convert_memo.insert(id, out);
            return Ok(out);
        }

        if let Some((var, body)) = match_sym_rec(self.arena, id) {
            let converted_body = self.convert(body)?;
            let out = sym_rec(self.arena, var, converted_body);
            self.convert_memo.insert(id, out);
            return Ok(out);
        }

        if match_sym_ref(self.arena, id).is_some() {
            self.convert_memo.insert(id, id);
            return Ok(id);
        }

        if let Some(level) = match_de_bruijn_ref(self.arena, id) {
            return Err(RecursionError::UnboundDeBruijnReference { node: id, level });
        }
        if is_de_bruijn_ref_tag(self.arena, id) {
            return Err(RecursionError::MalformedDeBruijnReference { node: id });
        }

        let mut converted_children = Vec::with_capacity(node.children.len());
        for child in node.children.as_slice() {
            converted_children.push(self.convert(*child)?);
        }
        let out = self.arena.intern(node.kind, &converted_children);
        self.convert_memo.insert(id, out);
        Ok(out)
    }

    /// Substitutes the de Bruijn reference at exact `level` with `replacement`.
    ///
    /// This mirrors the C++ `substitute` helper:
    /// - `DEBRUIJNREF(level)` matching the requested level is replaced,
    /// - binder bodies recurse with `level + 1`,
    /// - nodes whose aperture is already below `level` are reused unchanged.
    fn substitute(
        &mut self,
        id: TreeId,
        level: i64,
        replacement: TreeId,
    ) -> Result<TreeId, RecursionError> {
        let key = (id, level, replacement);
        if let Some(mapped) = self.substitute_memo.get(&key) {
            return Ok(*mapped);
        }

        if self.aperture(id)? < level {
            self.substitute_memo.insert(key, id);
            return Ok(id);
        }

        if let Some(ref_level) = match_de_bruijn_ref(self.arena, id) {
            let out = if ref_level == level { replacement } else { id };
            self.substitute_memo.insert(key, out);
            return Ok(out);
        }
        if is_de_bruijn_ref_tag(self.arena, id) {
            return Err(RecursionError::MalformedDeBruijnReference { node: id });
        }

        if let Some(body) = match_de_bruijn_rec(self.arena, id) {
            let sub = self.substitute(body, level + 1, replacement)?;
            let out = de_bruijn_rec(self.arena, sub);
            self.substitute_memo.insert(key, out);
            return Ok(out);
        }

        let Some(node) = self.arena.node(id).cloned() else {
            return Err(RecursionError::InvalidNode { node: id });
        };
        let mut out_children = Vec::with_capacity(node.children.len());
        for child in node.children.as_slice() {
            out_children.push(self.substitute(*child, level, replacement)?);
        }
        let out = self.arena.intern(node.kind, &out_children);
        self.substitute_memo.insert(key, out);
        Ok(out)
    }

    /// Computes aperture with error reporting for malformed references/invalid nodes.
    ///
    /// Unlike the public [`de_bruijn_aperture`], this variant participates in
    /// conversion and therefore reports malformed trees instead of silently
    /// treating missing payloads as ordinary nodes.
    fn aperture(&mut self, id: TreeId) -> Result<i64, RecursionError> {
        if let Some(value) = self.aperture_memo.get(&id) {
            return Ok(*value);
        }

        if let Some(level) = match_de_bruijn_ref(self.arena, id) {
            self.aperture_memo.insert(id, level);
            return Ok(level);
        }
        if is_de_bruijn_ref_tag(self.arena, id) {
            return Err(RecursionError::MalformedDeBruijnReference { node: id });
        }

        let value = if let Some(body) = match_de_bruijn_rec(self.arena, id) {
            self.aperture(body)? - 1
        } else {
            let Some(children) = self.arena.children(id).map(|ch| ch.to_vec()) else {
                return Err(RecursionError::InvalidNode { node: id });
            };
            let mut max_aperture = 0;
            for child in children {
                let child_aperture = self.aperture(child)?;
                if child_aperture > max_aperture {
                    max_aperture = child_aperture;
                }
            }
            max_aperture
        };

        self.aperture_memo.insert(id, value);
        Ok(value)
    }
}

/// Stateful helper implementing `liftn`.
///
/// `threshold` tracks how many binders must be crossed before a reference is
/// considered free and therefore needs to be incremented.
struct Lifter<'a> {
    arena: &'a mut TreeArena,
    memo: AHashMap<(TreeId, i64), TreeId>,
}

impl<'a> Lifter<'a> {
    /// Creates one memoizing lifter scoped to a single arena/root call.
    fn new(arena: &'a mut TreeArena) -> Self {
        Self {
            arena,
            memo: AHashMap::new(),
        }
    }

    /// Lifts all free references with `level >= threshold` by one.
    ///
    /// Binder bodies recurse with `threshold + 1`, which preserves binding for
    /// locally bound references while shifting only the free part of the tree.
    fn lift(&mut self, id: TreeId, threshold: i64) -> TreeId {
        let key = (id, threshold);
        if let Some(mapped) = self.memo.get(&key) {
            return *mapped;
        }

        let out = if let Some(level) = match_de_bruijn_ref(self.arena, id) {
            if level < threshold {
                id
            } else {
                de_bruijn_ref(self.arena, level + 1)
            }
        } else if let Some(body) = match_de_bruijn_rec(self.arena, id) {
            let lifted = self.lift(body, threshold + 1);
            de_bruijn_rec(self.arena, lifted)
        } else if let Some(node) = self.arena.node(id).cloned() {
            let mut children = Vec::with_capacity(node.children.len());
            for child in node.children.as_slice() {
                children.push(self.lift(*child, threshold));
            }
            self.arena.intern(node.kind, &children)
        } else {
            id
        };

        self.memo.insert(key, out);
        out
    }
}

/// Memoized aperture worker used by the public read-only aperture query.
///
/// This variant is deliberately permissive: malformed `DEBRUIJNREF` payloads
/// simply behave like ordinary nodes because the public helper returns only an
/// integer aperture, not a `Result`.
fn aperture(arena: &TreeArena, root: TreeId, memo: &mut AHashMap<TreeId, i64>) -> i64 {
    if let Some(value) = memo.get(&root) {
        return *value;
    }

    let value = if let Some(level) = match_de_bruijn_ref(arena, root) {
        level
    } else if let Some(body) = match_de_bruijn_rec(arena, root) {
        aperture(arena, body, memo) - 1
    } else {
        let mut max_aperture = 0;
        if let Some(children) = arena.children(root) {
            for child in children {
                let child_aperture = aperture(arena, *child, memo);
                if child_aperture > max_aperture {
                    max_aperture = child_aperture;
                }
            }
        }
        max_aperture
    };

    memo.insert(root, value);
    value
}

fn check_coherence_at_depth(
    arena: &TreeArena,
    id: TreeId,
    depth: i64,
    memo: &mut AHashMap<(TreeId, i64), ()>,
) -> Result<(), RecursionError> {
    if memo.contains_key(&(id, depth)) {
        return Ok(());
    }

    if let Some(level) = match_de_bruijn_ref(arena, id) {
        if level <= 0 || level > depth {
            return Err(RecursionError::IncoherentDeBruijnReference {
                node: id,
                level,
                depth,
            });
        }
        memo.insert((id, depth), ());
        return Ok(());
    }
    if is_de_bruijn_ref_tag(arena, id) {
        return Err(RecursionError::MalformedDeBruijnReference { node: id });
    }

    if let Some(body) = match_de_bruijn_rec(arena, id) {
        check_coherence_at_depth(arena, body, depth + 1, memo)?;
        memo.insert((id, depth), ());
        return Ok(());
    }

    let Some(children) = arena.children(id).map(|ch| ch.to_vec()) else {
        return Err(RecursionError::InvalidNode { node: id });
    };
    for child in children {
        check_coherence_at_depth(arena, child, depth, memo)?;
    }
    memo.insert((id, depth), ());
    Ok(())
}

/// Returns children when `id` is a tag node with the exact expected tag name.
///
/// This small matcher centralizes the "tag name + arity checked by caller"
/// pattern shared by all public recursion matchers.
fn tag_children<'a>(arena: &'a TreeArena, id: TreeId, expected_tag: &str) -> Option<&'a [TreeId]> {
    let node = arena.node(id)?;
    let NodeKind::Tag(tag_id) = &node.kind else {
        return None;
    };
    if arena.tag_name(*tag_id)? != expected_tag {
        return None;
    }
    Some(node.children.as_slice())
}

/// Returns whether `id` is tagged as `DEBRUIJNREF`, regardless of payload shape.
///
/// This is used to distinguish a malformed reference payload from a completely
/// unrelated node kind when producing structured errors.
fn is_de_bruijn_ref_tag(arena: &TreeArena, id: TreeId) -> bool {
    let Some(node) = arena.node(id) else {
        return false;
    };
    let NodeKind::Tag(tag_id) = &node.kind else {
        return false;
    };
    arena.tag_name(*tag_id) == Some(DEBRUIJNREF_TAG)
}

/// Interns one tag node by tag name plus ordered children.
///
/// This keeps all recursion-specific builders on the same hash-consing path as
/// the rest of `TreeArena`.
fn intern_tag(arena: &mut TreeArena, tag: &str, children: &[TreeId]) -> TreeId {
    let tag_id = arena.intern_tag(tag);
    arena.intern(NodeKind::Tag(tag_id), children)
}

#[cfg(test)]
mod tests {
    use super::{de_bruijn_rec, de_bruijn_ref, de_bruijn_to_sym, match_sym_rec, sym_rec};
    use crate::{TreeArena, list_to_vec};

    #[test]
    fn de_bruijn_to_sym_skips_preexisting_symbol_name_collisions() {
        let mut arena = TreeArena::new();

        let colliding_var = arena.symbol("W0");
        let zero = arena.int(0);
        let nil = arena.nil();
        let preexisting_body = arena.cons(zero, nil);
        let _preexisting = sym_rec(&mut arena, colliding_var, preexisting_body);

        let ref1 = de_bruijn_ref(&mut arena, 1);
        let nil = arena.nil();
        let body = arena.cons(ref1, nil);
        let rec = de_bruijn_rec(&mut arena, body);

        let converted = de_bruijn_to_sym(&mut arena, rec).expect("closed de Bruijn tree");
        let (fresh_var, body_list) = match_sym_rec(&arena, converted).expect("symbolic recursion");
        assert_ne!(fresh_var, colliding_var);
        assert_eq!(list_to_vec(&arena, body_list).expect("body list").len(), 1);
    }
}
