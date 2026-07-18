//! Compute-region tree for `compute()` assembly (roadmap P2).
//!
//! # Design note (P2.1)
//!
//! The FIR `compute()` body is assembled from a tree of **regions**. A region
//! is a lexical execution scope with its own statement lists; the full model
//! (from `porting/ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md` §6
//! and the 2026-06-10 roadmap §5) is:
//!
//! - **scalar mode**: `SampleLoop ⊃ GuardedBlock(OD/US/DS, nested)` — one
//!   guarded region per clocked-wrapper instance, strictly nested like the
//!   clock domains themselves (roadmap P3);
//! - **vector mode**: `ChunkLoop ⊃ { LoopNode | Island ⊃ GuardedBlock }`
//!   (roadmap P6/P7).
//!
//! **The visibility rule** (single rule for CSE / occurrences / placement):
//! *a value computed in region `R` is reusable only in `R` and its
//! descendants; cross-region reuse goes through named storage* (stack locals,
//! struct fields, chunk buffers). While the tree holds exactly one region per
//! sample loop (the P2.2 state), per-bucket CSE (`cse.rs`) and
//! variability-driven placement (`placement.rs`) already respect the rule
//! trivially — bucket and region coincide. Guarded child regions (P3) extend
//! the rule by *narrowing* reuse, never widening it.
//!
//! **FIR-vocabulary decision** (P2.1 checklist): guarded blocks reuse the
//! existing generic `If` / `SimpleForLoop` / `Block` FIR statements (the
//! vector-doc §4 finding) rather than introducing dedicated block nodes;
//! this module therefore stays a pure *assembly-side* structure with no new
//! FIR node kinds.
//!
//! # Current state (P2.2)
//!
//! [`RegionTree`] is instantiated with exactly one [`RegionKind::SampleLoop`]
//! region, plus the reverse-time loop as a sibling region opened by
//! `reset_sample_loop_state`. The tree API is the **only** way lowering code
//! appends compute statements (the P2 exit criterion): the former flat
//! `SamplePhases` accumulator lives on as [`RegionPhases`], owned by the
//! current region.

use std::collections::HashMap;

use fir::FirId;
use signals::SigId;

/// Region-scoped signal memoization.
///
/// Scope zero belongs to the current top-level sample loop. Each open guarded
/// child owns one additional scope. Lookup walks from the effective append
/// scope towards its ancestors, which implements the region visibility rule:
/// parent values are reusable by descendants, but child values never escape
/// into siblings. An insertion redirected to an ancestor is stored directly
/// in that ancestor scope and therefore survives closing the child.
pub(super) struct RegionCache {
    scopes: Vec<HashMap<SigId, FirId>>,
}

impl RegionCache {
    pub(super) fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    pub(super) fn get_at(&self, depth: usize, sig: SigId) -> Option<FirId> {
        debug_assert!(depth < self.scopes.len(), "cache depth must be open");
        self.scopes[..=depth]
            .iter()
            .rev()
            .find_map(|scope| scope.get(&sig).copied())
    }

    pub(super) fn insert_at(&mut self, depth: usize, sig: SigId, value: FirId) -> Option<FirId> {
        self.scopes
            .get_mut(depth)
            .expect("cache insertion depth must be open")
            .insert(sig, value)
    }

    pub(super) fn open_child(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub(super) fn close_child(&mut self) {
        assert!(
            self.scopes.len() > 1,
            "close_child requires an open cache scope"
        );
        self.scopes.pop();
    }

    pub(super) fn clear(&mut self) {
        debug_assert_eq!(
            self.scopes.len(),
            1,
            "clearing the loop cache with open guarded children"
        );
        self.scopes[0].clear();
    }
}

/// Kind of one compute region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegionKind {
    /// The forward per-sample loop body.
    SampleLoop,
    /// The reverse-time per-sample loop body (public RAD outputs).
    ReverseSampleLoop,
}

/// Explicit execution phases inside one region iteration.
///
/// The region body is assembled in this fixed order:
///
/// 1. `immediate`: ordinary per-sample work and writes that must happen before
///    outputs are finalized
/// 2. `post_output`: updates that must observe the current sample's outputs
///    before shifting/finalizing state
/// 3. `sample_end`: generic subsystem maintenance such as delay counter bumps
#[derive(Default)]
pub(super) struct RegionPhases {
    pub(super) immediate: Vec<FirId>,
    pub(super) post_output: Vec<FirId>,
    pub(super) sample_end: Vec<FirId>,
}

impl RegionPhases {
    /// Concatenates the three lifecycle phases into a single statement list,
    /// preserving execution order: `immediate`, then `post_output`, then
    /// `sample_end`.
    pub(super) fn flattened(&self) -> Vec<FirId> {
        let mut all = Vec::with_capacity(
            self.immediate.len() + self.post_output.len() + self.sample_end.len(),
        );
        all.extend(self.immediate.iter().copied());
        all.extend(self.post_output.iter().copied());
        all.extend(self.sample_end.iter().copied());
        all
    }
}

/// One compute region: a lexical scope with its own phased statement lists.
///
/// Child regions (guarded OD/US/DS blocks) arrive with P3; until then a
/// region is a leaf.
struct Region {
    #[allow(dead_code, reason = "read once guarded blocks (P3) emit per-kind")]
    kind: RegionKind,
    phases: RegionPhases,
}

/// The compute-region tree, with a cursor designating the region lowering
/// code currently appends into.
///
/// Regions at the top level are the sibling per-loop slices (forward sample
/// loop, reverse-time loop) emitted in creation order by `build_module`.
/// Guarded clocked blocks (P3) nest as a stack of **open child frames** under
/// the current top-level region; the innermost open frame is the default
/// append target.
///
/// # Redirection
/// Clocked lowering must place a signal computed in an *ancestor* domain in
/// the ancestor's region even when a descendant block is currently open (the
/// visibility rule of the P2.1 design note, applied in the emission
/// direction). [`Self::set_redirect`] retargets the append surface to an
/// outer depth: `0` = the current top-level region, `k > 0` = the `k`-th open
/// child frame. Non-clocked lowering never opens children nor redirects, so
/// the P2.2 behavior is unchanged.
pub(super) struct RegionTree {
    regions: Vec<Region>,
    current: usize,
    /// Open nested guarded-block frames, innermost last.
    open_children: Vec<RegionPhases>,
    /// Append redirection depth (see type docs); `None` = innermost.
    redirect: Option<usize>,
}

impl RegionTree {
    /// Creates a tree holding one open region of the given kind.
    pub(super) fn new(kind: RegionKind) -> Self {
        Self {
            regions: vec![Region {
                kind,
                phases: RegionPhases::default(),
            }],
            current: 0,
            open_children: Vec::new(),
            redirect: None,
        }
    }

    /// Phased statement lists of the append-target region — the single
    /// append surface for all lowering code.
    pub(super) fn current_phases_mut(&mut self) -> &mut RegionPhases {
        let depth = self.redirect.unwrap_or(self.open_children.len());
        if depth == 0 {
            &mut self.regions[self.current].phases
        } else {
            &mut self.open_children[depth - 1]
        }
    }

    /// Flattens the current top-level region's phases into one ordered
    /// statement list.
    pub(super) fn current_flattened(&self) -> Vec<FirId> {
        debug_assert!(
            self.open_children.is_empty(),
            "flattening a loop slice with unclosed guarded blocks"
        );
        self.regions[self.current].phases.flattened()
    }

    /// Closes the current region and opens a fresh sibling of `kind`,
    /// pointing the cursor at it.
    pub(super) fn begin_sibling(&mut self, kind: RegionKind) {
        debug_assert!(
            self.open_children.is_empty(),
            "starting a new loop slice with unclosed guarded blocks"
        );
        self.regions.push(Region {
            kind,
            phases: RegionPhases::default(),
        });
        self.current = self.regions.len() - 1;
    }

    /// Number of open guarded-block frames.
    pub(super) fn child_depth(&self) -> usize {
        self.open_children.len()
    }

    /// Depth of the region that currently receives appended statements.
    pub(super) fn effective_depth(&self) -> usize {
        self.redirect.unwrap_or(self.open_children.len())
    }

    /// Opens a nested guarded-block frame; it becomes the append target
    /// (unless a redirection is active).
    pub(super) fn open_child(&mut self) {
        self.open_children.push(RegionPhases::default());
    }

    /// Closes the innermost guarded-block frame and returns its phases so
    /// the caller can wrap them in the guard statement.
    pub(super) fn close_child(&mut self) -> RegionPhases {
        self.open_children
            .pop()
            .expect("close_child requires an open guarded-block frame")
    }

    /// Retargets the append surface to `depth` (see type docs); returns the
    /// previous redirection so callers can restore it.
    pub(super) fn set_redirect(&mut self, depth: Option<usize>) -> Option<usize> {
        std::mem::replace(&mut self.redirect, depth)
    }

    /// Current redirection depth, if a redirection is active.
    pub(super) fn redirect_depth(&self) -> Option<usize> {
        self.redirect
    }
}

#[cfg(test)]
mod tests {
    use super::RegionCache;
    use tlib::TreeArena;

    #[test]
    fn parent_entries_are_visible_in_children() {
        let mut cache = RegionCache::new();
        let mut arena = TreeArena::new();
        let signal = arena.int(1);
        let value = arena.int(10);
        cache.insert_at(0, signal, value);
        cache.open_child();

        assert_eq!(cache.get_at(1, signal), Some(value));
    }

    #[test]
    fn child_entries_do_not_escape_to_siblings() {
        let mut cache = RegionCache::new();
        let mut arena = TreeArena::new();
        let signal = arena.int(1);
        let value = arena.int(10);
        cache.open_child();
        cache.insert_at(1, signal, value);
        cache.close_child();
        cache.open_child();

        assert_eq!(cache.get_at(1, signal), None);
    }

    #[test]
    fn redirected_parent_entries_survive_child_close() {
        let mut cache = RegionCache::new();
        let mut arena = TreeArena::new();
        let signal = arena.int(1);
        let value = arena.int(10);
        cache.open_child();
        cache.insert_at(0, signal, value);
        cache.close_child();

        assert_eq!(cache.get_at(0, signal), Some(value));
    }
}
