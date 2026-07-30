//! Source-neutral provenance carried beside generated FIR.
//!
//! C++ Faust commonly relies on mutable properties attached to shared Tree
//! nodes. Rust keeps diagnostic identity out of hash-consed FIR identity:
//! [`FirOrigins`] records which prepared Signal nodes produced each [`FirId`]
//! and snapshots their Box derivations. This lets the compiler facade join a
//! verifier/backend failure back to parser occurrences without making source
//! locations affect FIR equality.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use boxes::BoxId;
use fir::{FirId, FirStore, fir_match_children};
use propagate::SignalOrigins;
use signals::SigId;

/// One Signal derivation attached to a FIR node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirSignalOrigin {
    /// Store-local prepared Signal id, retained as bounded debug evidence.
    pub signal: SigId,
    /// Ordered Box candidates from propagation/preparation provenance.
    pub boxes: Vec<BoxId>,
}

/// Explicit `FirId -> Signal/Box derivations` side table.
///
/// The table is deliberately separate from [`FirStore`]. Identical FIR nodes
/// remain hash-consed; when several Signal occurrences produce the same node,
/// their derivations accumulate in deterministic Signal-id order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FirOrigins {
    by_fir: BTreeMap<FirId, Vec<FirSignalOrigin>>,
}

impl FirOrigins {
    /// Creates an empty provenance table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Maximum Signal derivations retained per FIR node.
    ///
    /// Like [`SignalOrigins::MAX_ORIGINS_PER_SIGNAL`], this keeps the tables
    /// bounded diagnostic evidence: `derive_reachable` unions descendant
    /// derivations into every reachable parent, so unbounded per-node lists
    /// would grow with program size and make the walk super-quadratic.
    pub const MAX_SIGNALS_PER_NODE: usize = 8;

    /// Records one prepared Signal as a direct producer of `fir`.
    ///
    /// Retains at most [`Self::MAX_SIGNALS_PER_NODE`] derivations per node,
    /// in Signal-id order.
    pub fn record_signal(&mut self, fir: FirId, signal: SigId, signal_origins: &SignalOrigins) {
        let boxes = signal_origins.origins_for(signal).to_vec();
        let entries = self.by_fir.entry(fir).or_default();
        if let Some(existing) = entries.iter_mut().find(|entry| entry.signal == signal) {
            for origin in boxes {
                if !existing.boxes.contains(&origin) {
                    existing.boxes.push(origin);
                }
            }
        } else if entries.len() < Self::MAX_SIGNALS_PER_NODE {
            entries.push(FirSignalOrigin { signal, boxes });
            entries.sort_by_key(|entry| entry.signal.as_u32());
        }
    }

    /// Returns derivations associated with `fir`.
    #[must_use]
    pub fn origins_for(&self, fir: FirId) -> &[FirSignalOrigin] {
        self.by_fir.get(&fir).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Returns ordered, deduplicated Box candidates associated with `fir`.
    #[must_use]
    pub fn box_origins_for(&self, fir: FirId) -> Vec<BoxId> {
        let mut seen = BTreeSet::new();
        self.origins_for(fir)
            .iter()
            .flat_map(|entry| entry.boxes.iter().copied())
            .filter(|origin| seen.insert(origin.as_u32()))
            .collect()
    }

    /// Propagates child derivations to every reachable FIR parent.
    ///
    /// Run this after module assembly or a rewrite pass. Statements, blocks,
    /// lifecycle functions, and the module root then inherit the union of the
    /// values/statements they contain.
    pub fn derive_reachable(&mut self, store: &FirStore, root: FirId) {
        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        self.derive_node(store, root, &mut visiting, &mut visited);
    }

    /// Remaps direct and derived origins through a FIR clone/rewrite mapping.
    ///
    /// `pairs` may contain one source id several times when a scoped clone
    /// duplicates a shared node. Every destination receives the same ordered
    /// origin set.
    #[must_use]
    pub fn remap_pairs(&self, pairs: &[(FirId, FirId)]) -> Self {
        let mut remapped = Self::new();
        for &(source, destination) in pairs {
            for origin in self.origins_for(source) {
                merge_origin(remapped.by_fir.entry(destination).or_default(), origin);
            }
        }
        remapped
    }

    fn derive_node(
        &mut self,
        store: &FirStore,
        node: FirId,
        visiting: &mut HashSet<FirId>,
        visited: &mut HashSet<FirId>,
    ) -> Vec<FirSignalOrigin> {
        if visited.contains(&node) {
            return self.by_fir.get(&node).cloned().unwrap_or_default();
        }
        if !visiting.insert(node) {
            return self.by_fir.get(&node).cloned().unwrap_or_default();
        }
        let mut combined = self.by_fir.get(&node).cloned().unwrap_or_default();
        for child in fir_match_children(store, node) {
            for origin in self.derive_node(store, child, visiting, visited) {
                merge_origin(&mut combined, &origin);
            }
        }
        visiting.remove(&node);
        visited.insert(node);
        if !combined.is_empty() {
            self.by_fir.insert(node, combined.clone());
        }
        combined
    }
}

fn merge_origin(entries: &mut Vec<FirSignalOrigin>, origin: &FirSignalOrigin) {
    if let Some(existing) = entries
        .iter_mut()
        .find(|entry| entry.signal == origin.signal)
    {
        for &box_origin in &origin.boxes {
            if !existing.boxes.contains(&box_origin) {
                existing.boxes.push(box_origin);
            }
        }
    } else if entries.len() < FirOrigins::MAX_SIGNALS_PER_NODE {
        entries.push(origin.clone());
        entries.sort_by_key(|entry| entry.signal.as_u32());
    }
}

#[cfg(test)]
mod tests {
    use super::FirOrigins;
    use boxes::BoxBuilder;
    use fir::{FirBuilder, FirStore};
    use propagate::SignalOrigins;
    use signals::SigBuilder;
    use tlib::TreeArena;

    #[test]
    fn shared_fir_nodes_accumulate_signals_and_flow_to_statements() {
        let mut box_arena = TreeArena::new();
        let (box_a, box_b) = {
            let mut builder = BoxBuilder::new(&mut box_arena);
            (builder.int(1), builder.int(2))
        };
        let mut signal_arena = TreeArena::new();
        let (signal_a, signal_b) = {
            let mut builder = SigBuilder::new(&mut signal_arena);
            (builder.int(1), builder.int(2))
        };
        let mut signal_origins = SignalOrigins::default();
        signal_origins.record(signal_a, box_a);
        signal_origins.record(signal_b, box_b);

        let mut store = FirStore::new();
        let (shared, root) = {
            let mut builder = FirBuilder::new(&mut store);
            let shared = builder.int32(7);
            let dropped = builder.drop_(shared);
            (shared, builder.block(&[dropped]))
        };

        let mut origins = FirOrigins::new();
        origins.record_signal(shared, signal_a, &signal_origins);
        origins.record_signal(shared, signal_b, &signal_origins);
        origins.derive_reachable(&store, root);

        assert_eq!(origins.origins_for(shared).len(), 2);
        assert_eq!(origins.box_origins_for(root), vec![box_a, box_b]);
    }
}
