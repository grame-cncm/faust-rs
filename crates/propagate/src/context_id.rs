//! Canonical identities for propagation context values.
//!
//! C++ source provenance:
//! - `compiler/propagate/propagate.cpp` passes `slotenv` and `path` through
//!   `propagate(...)` and uses their hash-consed `Tree*` values in the result
//!   memo key.
//! - `compiler/tlib/tree.cpp` supplies the canonical tree identities that make
//!   those key fields pointer-sized and allocation-free to compare.
//!
//! Rust cannot use the former mutable `AHashMap<BoxId, SigId>` or owned UI path
//! directly in an equally cheap key. This module adapts the C++ representation:
//! slot bindings form an interned persistent chain, while normalized UI paths
//! are interned once per propagation run. Equal construction histories receive
//! equal compact ids. The ids are introduced independently of result
//! memoization so their restoration and shadowing semantics can be tested first.

use ahash::AHashMap;
use boxes::BoxId;
use signals::SigId;
use ui::UiGroupPathSegment;

/// Canonical identity of one persistent slot environment.
///
/// `EMPTY` represents no bindings. Every other id denotes exactly one
/// `(parent, slot, signal)` node interned for the current propagation run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct SlotEnvId(u32);

impl SlotEnvId {
    const EMPTY: Self = Self(0);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SlotBindingKey {
    parent: SlotEnvId,
    slot: BoxId,
    signal: SigId,
}

#[derive(Clone, Copy, Debug)]
struct SlotBinding {
    key: SlotBindingKey,
    visible_len: usize,
}

/// Persistent, compilation-scoped slot environment with canonical ids.
///
/// Binding is O(1) after hash lookup, restoration is a single id assignment,
/// and lookup walks the usually tiny lexical chain from newest to oldest. A
/// shadowed binding remains in the persistent parent chain, which preserves the
/// exact lexical construction context while returning only the newest value.
pub(crate) struct SlotEnv {
    current: SlotEnvId,
    nodes: Vec<SlotBinding>,
    interner: AHashMap<SlotBindingKey, SlotEnvId>,
}

impl Default for SlotEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl SlotEnv {
    pub(crate) fn new() -> Self {
        Self {
            current: SlotEnvId::EMPTY,
            nodes: Vec::new(),
            interner: AHashMap::new(),
        }
    }

    /// Returns the canonical identity of the active environment.
    #[inline]
    pub(crate) const fn id(&self) -> SlotEnvId {
        self.current
    }

    /// Returns the number of visible (non-shadowed) bindings.
    pub(crate) fn len(&self) -> usize {
        self.node(self.current).map_or(0, |node| node.visible_len)
    }

    /// Looks up the newest value bound to `slot`.
    pub(crate) fn get(&self, slot: &BoxId) -> Option<SigId> {
        let mut cursor = self.current;
        while let Some(node) = self.node(cursor) {
            if node.key.slot == *slot {
                return Some(node.key.signal);
            }
            cursor = node.key.parent;
        }
        None
    }

    /// Binds `slot` and returns the identity to restore when leaving its scope.
    pub(crate) fn push(&mut self, slot: BoxId, signal: SigId) -> SlotEnvId {
        let saved = self.current;
        self.current = self.intern_binding(saved, slot, signal);
        saved
    }

    /// Restores a previously returned environment identity.
    #[inline]
    pub(crate) fn restore(&mut self, id: SlotEnvId) {
        debug_assert!(id == SlotEnvId::EMPTY || (id.0 as usize) <= self.nodes.len());
        self.current = id;
    }

    /// Returns the active binding chain in oldest-to-newest order.
    ///
    /// The chain intentionally includes shadowed parents. Rebuilding a lifted
    /// recursion context from it therefore preserves the same lexical history
    /// and produces a stable canonical identity on repeated construction.
    pub(crate) fn binding_chain(&self) -> Vec<(BoxId, SigId)> {
        let mut bindings = Vec::new();
        let mut cursor = self.current;
        while let Some(node) = self.node(cursor) {
            bindings.push((node.key.slot, node.key.signal));
            cursor = node.key.parent;
        }
        bindings.reverse();
        bindings
    }

    /// Interns a complete oldest-to-newest binding chain and makes it active.
    pub(crate) fn replace_with_chain(&mut self, bindings: &[(BoxId, SigId)]) -> SlotEnvId {
        let saved = self.current;
        let mut current = SlotEnvId::EMPTY;
        for &(slot, signal) in bindings {
            current = self.intern_binding(current, slot, signal);
        }
        self.current = current;
        saved
    }

    fn intern_binding(&mut self, parent: SlotEnvId, slot: BoxId, signal: SigId) -> SlotEnvId {
        let key = SlotBindingKey {
            parent,
            slot,
            signal,
        };
        if let Some(id) = self.interner.get(&key).copied() {
            return id;
        }
        let visible_len = self.node(parent).map_or(0, |node| node.visible_len)
            + usize::from(self.get_from(parent, slot).is_none());
        let id = SlotEnvId(
            u32::try_from(self.nodes.len() + 1)
                .expect("one propagation cannot intern more than u32::MAX slot contexts"),
        );
        self.nodes.push(SlotBinding { key, visible_len });
        self.interner.insert(key, id);
        id
    }

    fn get_from(&self, mut cursor: SlotEnvId, slot: BoxId) -> Option<SigId> {
        while let Some(node) = self.node(cursor) {
            if node.key.slot == slot {
                return Some(node.key.signal);
            }
            cursor = node.key.parent;
        }
        None
    }

    fn node(&self, id: SlotEnvId) -> Option<&SlotBinding> {
        id.0.checked_sub(1)
            .map(|index| &self.nodes[usize::try_from(index).expect("u32 slot id must fit usize")])
    }
}

/// Canonical identity of one normalized grouped-UI path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct UiPathId(u32);

impl UiPathId {
    const EMPTY: Self = Self(0);
}

/// Saved UI-path state used for lexical restoration.
pub(crate) struct SavedUiPath {
    groups: Vec<UiGroupPathSegment>,
    id: UiPathId,
}

/// Current normalized UI path plus its compilation-scoped canonical identity.
pub(crate) struct UiPathContext {
    groups: Vec<UiGroupPathSegment>,
    id: UiPathId,
    interner: AHashMap<Vec<UiGroupPathSegment>, UiPathId>,
}

impl Default for UiPathContext {
    fn default() -> Self {
        Self::new()
    }
}

impl UiPathContext {
    pub(crate) fn new() -> Self {
        Self {
            groups: Vec::new(),
            id: UiPathId::EMPTY,
            interner: AHashMap::new(),
        }
    }

    /// Returns the normalized path used by UI label resolution.
    #[inline]
    pub(crate) fn groups(&self) -> &[UiGroupPathSegment] {
        &self.groups
    }

    /// Returns the canonical identity of the current normalized path.
    #[inline]
    pub(crate) const fn id(&self) -> UiPathId {
        self.id
    }

    /// Replaces the active path and returns state for exact restoration.
    pub(crate) fn replace(&mut self, groups: Vec<UiGroupPathSegment>) -> SavedUiPath {
        let id = self.intern(&groups);
        let saved = SavedUiPath {
            groups: std::mem::replace(&mut self.groups, groups),
            id: self.id,
        };
        self.id = id;
        saved
    }

    /// Temporarily switches to the empty UI path.
    pub(crate) fn clear(&mut self) -> SavedUiPath {
        self.replace(Vec::new())
    }

    /// Restores state returned by [`Self::replace`] or [`Self::clear`].
    pub(crate) fn restore(&mut self, saved: SavedUiPath) {
        self.groups = saved.groups;
        self.id = saved.id;
    }

    fn intern(&mut self, groups: &[UiGroupPathSegment]) -> UiPathId {
        if groups.is_empty() {
            return UiPathId::EMPTY;
        }
        if let Some(id) = self.interner.get(groups).copied() {
            return id;
        }
        let id = UiPathId(
            u32::try_from(self.interner.len() + 1)
                .expect("one propagation cannot intern more than u32::MAX UI paths"),
        );
        let owned = groups.to_vec();
        self.interner.insert(owned, id);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tlib::TreeArena;
    use ui::UiGroupKind;

    #[test]
    fn slot_environment_interns_equal_binding_histories() {
        let mut arena = TreeArena::new();
        let slot = arena.int(1);
        let signal = arena.int(2);
        let mut env = SlotEnv::new();

        let empty = env.push(slot, signal);
        let first = env.id();
        env.restore(empty);
        env.push(slot, signal);

        assert_eq!(env.id(), first);
        assert_eq!(env.get(&slot), Some(signal));
        assert_eq!(env.len(), 1);
    }

    #[test]
    fn slot_environment_shadowing_and_restoration_are_lexical() {
        let mut arena = TreeArena::new();
        let slot = arena.int(1);
        let outer_signal = arena.int(2);
        let inner_signal = arena.int(3);
        let mut env = SlotEnv::new();

        env.push(slot, outer_signal);
        let outer = env.id();
        let saved = env.push(slot, inner_signal);
        assert_eq!(saved, outer);
        assert_eq!(env.get(&slot), Some(inner_signal));
        assert_eq!(env.len(), 1);

        env.restore(saved);
        assert_eq!(env.id(), outer);
        assert_eq!(env.get(&slot), Some(outer_signal));
    }

    #[test]
    fn rebuilt_slot_chain_receives_the_same_identity() {
        let mut arena = TreeArena::new();
        let bindings = [(arena.int(1), arena.int(11)), (arena.int(2), arena.int(12))];
        let mut env = SlotEnv::new();

        env.replace_with_chain(&bindings);
        let first = env.id();
        env.restore(SlotEnvId::EMPTY);
        env.replace_with_chain(&bindings);

        assert_eq!(env.id(), first);
        assert_eq!(env.binding_chain(), bindings);
    }

    #[test]
    fn ui_paths_intern_equal_normalized_values_and_restore_exactly() {
        let group = UiGroupPathSegment {
            kind: UiGroupKind::Horizontal,
            raw_label: "amp".to_owned(),
        };
        let mut path = UiPathContext::new();

        let root = path.replace(vec![group.clone()]);
        let first = path.id();
        path.restore(root);
        assert_eq!(path.id(), UiPathId::EMPTY);

        let root_again = path.replace(vec![group]);
        assert_eq!(path.id(), first);
        path.restore(root_again);
        assert!(path.groups().is_empty());
    }
}
