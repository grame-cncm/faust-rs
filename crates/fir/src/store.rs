//! Hash-consed FIR storage.
//!
//! `FirStore` wraps the shared `TreeArena` representation used by the builder
//! and matcher. `FirId` handles are store-local and must not be mixed across
//! stores without explicit rebuilding.

use super::*;

/// FIR storage using `tlib::TreeArena` hash-consing.
///
/// `FirId`s are store-local handles. They must not be mixed across stores
/// without explicit rebuilding or cloning through a dedicated helper.
#[derive(Debug)]
pub struct FirStore {
    pub(crate) arena: TreeArena,
}

impl Default for FirStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FirStore {
    /// Creates a new instance of this type.
    #[must_use]
    pub fn new() -> Self {
        Self {
            arena: TreeArena::new(),
        }
    }

    /// Returns the number of elements currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.arena.len()
    }

    /// Returns `true` when there are no FIR nodes besides canonical `nil`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.arena.len() <= 1
    }

    /// Re-interns one subtree of `src` into this store and returns its local
    /// handle.
    ///
    /// [`FirId`]s are store-local, so a FIR tree built in a separate store —
    /// for instance a table generator lowered by its own `SignalToFirLower`,
    /// which owns its [`FirStore`] — must be imported before it can be
    /// referenced here. Hash-consing, ordered children, repeated-subtree
    /// sharing and tag identity are all preserved by the underlying arena
    /// primitive.
    #[must_use]
    pub fn import_from(&mut self, src: &Self, root: FirId) -> FirId {
        self.arena.clone_subtree_from(&src.arena, root)
    }

    /// Returns the value type when `id` points to a value node.
    #[must_use]
    pub fn value_type(&self, id: FirId) -> Option<FirType> {
        let node = self.arena.node(id)?;
        let NodeKind::Tag(tag_id) = &node.kind else {
            return None;
        };
        let tag = self.arena.tag_name(*tag_id)?;
        if !is_value_tag(tag) {
            return None;
        }
        let typ_id = *node.children.as_slice().first()?;
        decode_type(&self.arena, typ_id)
    }
}
