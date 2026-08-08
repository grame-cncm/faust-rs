//! Exact compilation-scoped memoization for Box-to-Signal propagation.
//!
//! C++ source provenance:
//! - `compiler/propagate/propagate.cpp`, where `propagate(...)` memoizes the
//!   exact tuple `(slotenv, path, box, inputs)` and returns the cached signal
//!   list before entering `realPropagate(...)`.
//! - `compiler/tlib/tree.cpp`, whose hash-consed `Tree*` values make slot and
//!   path context identity pointer-sized.
//!
//! The Rust adaptation uses [`SlotEnvId`](crate::context_id::SlotEnvId) and
//! [`UiPathId`](crate::context_id::UiPathId) for those canonical context
//! components. Zero-, one-, and two-signal buses are stored directly in the
//! key; larger buses are interned once per propagation run. A probe for the
//! overwhelmingly common small buses therefore neither allocates nor clones an
//! owned `Vec<SigId>`.
//!
//! Memoization is deliberately disabled for a complete propagation containing
//! forward/reverse AD or clocked wrappers. Those families carry pending-seed or
//! fresh clock-domain side effects that cannot yet be replayed from a signal
//! result alone. Provenance remains safe for eligible calls: the first miss
//! records every descendant `(signal, box)` derivation, and a later exact-key
//! hit can only replay the same canonical signals for the same Box nodes.
//! A 1,024-entry warm-up keeps small DSPs on the allocation-free uncached path;
//! large traversals activate the table only after they have demonstrated enough
//! work to amortize its hashing and retained buses.

use std::sync::Arc;

use ahash::{AHashMap, AHashSet};
use signals::SigId;
use tlib::{TreeArena, TreeId};

use crate::clock_domain::ClockDomainId;
use crate::context_id::{SlotEnvId, UiPathId};
use crate::{FlatBoxBuildError, FlatBoxId, FlatNodeKind, flat_node_kind};

const RESULT_MEMO_WARMUP_CALLS: u32 = 1_024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct BusId(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum BusKey {
    Empty,
    One(SigId),
    Two(SigId, SigId),
    Many(BusId),
}

struct BusInterner {
    buses: Vec<Arc<[SigId]>>,
    interner: AHashMap<Arc<[SigId]>, BusId>,
}

impl Default for BusInterner {
    fn default() -> Self {
        Self {
            buses: Vec::new(),
            interner: AHashMap::new(),
        }
    }
}

impl BusInterner {
    fn key(&mut self, signals: &[SigId]) -> BusKey {
        match signals {
            [] => BusKey::Empty,
            [signal] => BusKey::One(*signal),
            [first, second] => BusKey::Two(*first, *second),
            many => {
                if let Some(id) = self.interner.get(many).copied() {
                    return BusKey::Many(id);
                }
                let signals: Arc<[SigId]> = Arc::from(many);
                let id = BusId(
                    u32::try_from(self.buses.len())
                        .expect("one propagation cannot intern more than u32::MAX large buses"),
                );
                self.buses.push(Arc::clone(&signals));
                self.interner.insert(signals, id);
                BusKey::Many(id)
            }
        }
    }

    fn materialize(&self, key: BusKey) -> Vec<SigId> {
        match key {
            BusKey::Empty => Vec::new(),
            BusKey::One(signal) => vec![signal],
            BusKey::Two(first, second) => vec![first, second],
            BusKey::Many(id) => self.buses[id.0 as usize].to_vec(),
        }
    }
}

/// Output-affecting mutable propagation state represented in an exact key.
///
/// Clocked and AD roots are currently ineligible, but their fields remain part
/// of the key so enabling a side-effect replay protocol later cannot
/// accidentally alias nested rate or suppression contexts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PropagationModeKey {
    clock_env: TreeId,
    clock_domain: Option<ClockDomainId>,
    suppress_fad: bool,
}

impl PropagationModeKey {
    pub(crate) const fn new(
        clock_env: TreeId,
        clock_domain: Option<ClockDomainId>,
        suppress_fad: bool,
    ) -> Self {
        Self {
            clock_env,
            clock_domain,
            suppress_fad,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PropagateResultKey {
    box_tree: FlatBoxId,
    slot_env: SlotEnvId,
    ui_path: UiPathId,
    mode: PropagationModeKey,
    inputs: BusKey,
}

/// One-run exact result table adapted from C++ `gGlobal->gResult2Memo`.
pub(crate) struct PropagateResultMemo {
    safe_root: bool,
    eligible_calls: u32,
    buses: BusInterner,
    entries: AHashMap<PropagateResultKey, BusKey>,
}

impl Default for PropagateResultMemo {
    fn default() -> Self {
        Self {
            safe_root: false,
            eligible_calls: 0,
            buses: BusInterner::default(),
            entries: AHashMap::new(),
        }
    }
}

impl PropagateResultMemo {
    /// Enables the table only after whole-root side-effect analysis succeeds.
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.safe_root = enabled;
    }

    /// Constructs an exact, allocation-free key for buses of at most two
    /// signals. Larger buses allocate only on their first interning miss.
    pub(crate) fn key(
        &mut self,
        box_tree: FlatBoxId,
        slot_env: SlotEnvId,
        ui_path: UiPathId,
        mode: PropagationModeKey,
        inputs: &[SigId],
        has_slot_bindings: bool,
    ) -> Option<PropagateResultKey> {
        // The measurements that justify this table are symbolic recursive
        // subtrees. On ordinary closed graphs the key bookkeeping can cost
        // more than the shallow replay it finds, so retain the uncached path
        // until a lexical binding proves that the context-sensitive shape is
        // present.
        if !self.safe_root || !has_slot_bindings {
            return None;
        }
        self.eligible_calls = self.eligible_calls.saturating_add(1);
        if self.eligible_calls <= RESULT_MEMO_WARMUP_CALLS {
            return None;
        }
        Some(PropagateResultKey {
            box_tree,
            slot_env,
            ui_path,
            mode,
            inputs: self.buses.key(inputs),
        })
    }

    pub(crate) fn get(&self, key: PropagateResultKey) -> Option<Vec<SigId>> {
        self.entries
            .get(&key)
            .copied()
            .map(|outputs| self.buses.materialize(outputs))
    }

    pub(crate) fn insert(&mut self, key: PropagateResultKey, outputs: &[SigId]) {
        let outputs = self.buses.key(outputs);
        self.entries.insert(key, outputs);
    }
}

/// Returns whether exact result replay is side-effect safe for the whole root.
///
/// A visited set makes the analysis linear in the shared flat Box DAG. The
/// conservative whole-root gate can later become a per-subtree fact once AD
/// pending-seed and clock-domain deltas have an explicit replay protocol.
pub(crate) fn result_memo_is_safe_root(
    arena: &TreeArena,
    root: FlatBoxId,
) -> Result<bool, FlatBoxBuildError> {
    let mut pending = vec![root];
    let mut visited = AHashSet::new();
    while let Some(node) = pending.pop() {
        if !visited.insert(node) {
            continue;
        }
        match flat_node_kind(arena, node)? {
            FlatNodeKind::ForwardAD { .. }
            | FlatNodeKind::ReverseAD { .. }
            | FlatNodeKind::Ondemand(_)
            | FlatNodeKind::Upsampling(_)
            | FlatNodeKind::Downsampling(_) => return Ok(false),
            FlatNodeKind::Rec(left, right)
            | FlatNodeKind::Seq(left, right)
            | FlatNodeKind::Par(left, right)
            | FlatNodeKind::Split(left, right)
            | FlatNodeKind::Merge(left, right) => {
                pending.push(left);
                pending.push(right);
            }
            FlatNodeKind::Symbolic { body }
            | FlatNodeKind::Metadata { body }
            | FlatNodeKind::VGroup { body }
            | FlatNodeKind::HGroup { body }
            | FlatNodeKind::TGroup { body } => pending.push(body),
            _ => {}
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_id::{SlotEnv, UiPathContext};
    use boxes::BoxBuilder;
    use ui::{UiGroupKind, UiGroupPathSegment};

    #[test]
    fn small_bus_keys_are_exact_and_materialize_in_order() {
        let mut arena = TreeArena::new();
        let first = arena.int(1);
        let second = arena.int(2);
        let mut buses = BusInterner::default();

        let empty = buses.key(&[]);
        let one = buses.key(&[first]);
        let two = buses.key(&[first, second]);

        assert_eq!(buses.materialize(empty), Vec::<SigId>::new());
        assert_eq!(buses.materialize(one), vec![first]);
        assert_eq!(buses.materialize(two), vec![first, second]);
        assert!(buses.buses.is_empty());
    }

    #[test]
    fn large_equal_buses_receive_one_canonical_id() {
        let mut arena = TreeArena::new();
        let signals = [arena.int(1), arena.int(2), arena.int(3)];
        let mut buses = BusInterner::default();

        let first = buses.key(&signals);
        let second = buses.key(&signals);

        assert_eq!(first, second);
        assert_eq!(buses.materialize(first), signals);
        assert_eq!(buses.buses.len(), 1);
    }

    #[test]
    fn exact_key_distinguishes_slot_and_ui_contexts() {
        let mut arena = TreeArena::new();
        let raw_box = BoxBuilder::new(&mut arena).int(1);
        let box_tree = crate::try_build_flat_box(&arena, raw_box).expect("flat integer box");
        let signal = arena.int(2);
        let mut slots = SlotEnv::new();
        slots.push(arena.int(3), signal);
        let bound_slot = slots.id();
        slots.push(arena.int(4), signal);
        let nested_slot = slots.id();
        let mut ui = UiPathContext::new();
        let root_ui = ui.id();
        let saved_ui = ui.replace(vec![UiGroupPathSegment {
            kind: UiGroupKind::Horizontal,
            raw_label: "group".to_owned(),
        }]);
        let grouped_ui = ui.id();
        ui.restore(saved_ui);
        let mode = PropagationModeKey::new(arena.nil(), None, false);
        let mut memo = PropagateResultMemo::default();
        memo.set_enabled(true);

        for _ in 0..RESULT_MEMO_WARMUP_CALLS {
            assert!(
                memo.key(box_tree, bound_slot, root_ui, mode, &[signal], true)
                    .is_none()
            );
        }

        let root = memo
            .key(box_tree, bound_slot, root_ui, mode, &[signal], true)
            .expect("enabled key");
        let bound = memo
            .key(box_tree, nested_slot, root_ui, mode, &[signal], true)
            .expect("enabled key");
        let grouped = memo
            .key(box_tree, nested_slot, grouped_ui, mode, &[signal], true)
            .expect("enabled key");

        assert_ne!(root, bound);
        assert_ne!(root, grouped);
    }

    #[test]
    fn warmup_keeps_small_propagations_out_of_the_hash_table() {
        let mut arena = TreeArena::new();
        let raw_box = BoxBuilder::new(&mut arena).int(1);
        let box_tree = crate::try_build_flat_box(&arena, raw_box).expect("flat integer box");
        let mut slots = SlotEnv::new();
        let output = arena.int(42);
        slots.push(arena.int(3), output);
        let ui = UiPathContext::new();
        let mode = PropagationModeKey::new(arena.nil(), None, false);
        let mut memo = PropagateResultMemo::default();
        memo.set_enabled(true);

        for _ in 0..RESULT_MEMO_WARMUP_CALLS {
            assert!(
                memo.key(box_tree, slots.id(), ui.id(), mode, &[], true)
                    .is_none()
            );
        }
        assert!(memo.entries.is_empty());
        assert!(memo.buses.buses.is_empty());
        assert!(
            memo.key(box_tree, slots.id(), ui.id(), mode, &[], true)
                .is_some()
        );
    }

    #[test]
    fn root_safety_gate_excludes_ad_and_clock_side_effects() {
        let mut arena = TreeArena::new();
        let (plain, fad, clocked) = {
            let mut boxes = BoxBuilder::new(&mut arena);
            let wire = boxes.wire();
            (wire, boxes.forward_ad(wire, wire), boxes.ondemand(wire))
        };
        let plain = crate::try_build_flat_box(&arena, plain).expect("flat wire");
        let fad = crate::try_build_flat_box(&arena, fad).expect("flat FAD");
        let clocked = crate::try_build_flat_box(&arena, clocked).expect("flat clocked wrapper");

        assert!(result_memo_is_safe_root(&arena, plain).expect("plain analysis"));
        assert!(!result_memo_is_safe_root(&arena, fad).expect("FAD analysis"));
        assert!(!result_memo_is_safe_root(&arena, clocked).expect("clock analysis"));
    }
}
