//! Content identity of read-only generated tables (`DIFF-BEH-029`).
//!
//! A read-only table `WrTbl(size, Gen(g), nil, nil)` is declared, generated
//! and filled once per table node. Propagation annotates the stateful
//! primitives of a generator written inside a clock domain with that domain,
//! and the first element of a `waveform` (C++ `boxWaveform`), so one `rdtable`
//! read from several `ondemand` bodies is several table nodes that differ only
//! by those `Clocked(env, x)` annotations. Faust C++ emits one sub-container,
//! one waveform literal and one table per domain; so did both lowerers.
//!
//! The annotations cannot change the content: both fill paths drop every one
//! of them before computing it (`subcontainer_compile` prepares the generator
//! with `drop_clock_annotations`, and so does the `--table-init const`
//! interpreter, `siggen::interpret_generator`). Two generators equal once the
//! annotations are dropped therefore fill the same table. [`TableContentKeys`]
//! decides that equality, and the lowerers declare the table at its first node
//! and read it from the others.
//!
//! The prepared forest itself is not rewritten. Dropping the annotations there
//! would also change the stateful nodes a generator shares with the body it is
//! written in, which renames and adds state in programs that have nothing to
//! share; keying the declaration leaves every program without a duplicated
//! table byte-identical.
//!
//! Writable tables (`rwtable`) are never keyed: each one is per-instance state
//! written by its own clocked write port.

use std::collections::HashMap;

use fir::FirType;
use signals::{SigId, SigMatch, match_sig};
use tlib::{NodeKind, TreeArena};

/// Equality of read-only table contents modulo clock annotations, one
/// compiled module long.
///
/// The generator payloads are copied, without their `Clocked` wrappers, into
/// a private arena whose hash-consing then decides structural equality
/// exactly: no fingerprint, no collision to rule out.
#[derive(Default)]
pub(crate) struct TableContentKeys {
    /// Declocked copies of the generators seen so far.
    scratch: TreeArena,
    /// Source node to its declocked copy in `scratch`. Kept across tables:
    /// the copies of one waveform read in many domains share every element
    /// but the annotated first one.
    copies: HashMap<SigId, SigId>,
    /// `(size, element type, declocked generator)` to the first table node
    /// declared with that content.
    owners: HashMap<(usize, ElementKind, SigId), SigId>,
}

/// The element types a generated table can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ElementKind {
    Int32,
    Float32,
    Float64,
}

impl ElementKind {
    fn of(ty: &FirType) -> Option<Self> {
        match ty {
            FirType::Int32 => Some(Self::Int32),
            FirType::Float32 => Some(Self::Float32),
            FirType::Float64 => Some(Self::Float64),
            _ => None,
        }
    }
}

impl TableContentKeys {
    /// The table node whose declaration `table` must read.
    ///
    /// Returns the first node registered with the same size, element type and
    /// generator modulo clock annotations, or registers `table` as the owner of
    /// that content and returns it. `arena` is the arena `generator` lives in.
    pub(crate) fn owner(
        &mut self,
        arena: &TreeArena,
        table: SigId,
        size: usize,
        elem_ty: &FirType,
        generator: SigId,
    ) -> SigId {
        let Some(kind) = ElementKind::of(elem_ty) else {
            return table;
        };
        let content = self.declocked_copy(arena, generator);
        *self.owners.entry((size, kind, content)).or_insert(table)
    }

    /// Copies `sig` into the private arena without its `Clocked(env, x)`
    /// wrappers (the same rewrite `drop_clock_annotations` applies when a
    /// generator is compiled).
    fn declocked_copy(&mut self, arena: &TreeArena, sig: SigId) -> SigId {
        if let Some(copy) = self.copies.get(&sig) {
            return *copy;
        }
        let copy = if let SigMatch::Clocked(_, inner) = match_sig(arena, sig) {
            self.declocked_copy(arena, inner)
        } else if let Some(node) = arena.node(sig) {
            let children: Vec<SigId> = node
                .children
                .as_slice()
                .iter()
                .map(|&child| self.declocked_copy(arena, child))
                .collect();
            // Tag ids are arena-local: re-intern the tag by name, as
            // `TreeArena::clone_subtree_from` does.
            let kind = match &node.kind {
                NodeKind::Tag(tag) => match arena.tag_name(*tag) {
                    Some(name) => NodeKind::Tag(self.scratch.intern_tag(name)),
                    None => NodeKind::Tag(*tag),
                },
                other => other.clone(),
            };
            self.scratch.intern(kind, &children)
        } else {
            self.scratch.nil()
        };
        self.copies.insert(sig, copy);
        copy
    }
}

#[cfg(test)]
mod tests {
    use signals::SigBuilder;

    use super::*;

    /// `WrTbl(4, Gen(waveform{0.1@domain, 0.2, 0.3, 0.4}))`, the first element
    /// annotated with `domain` when there is one.
    fn table(arena: &mut TreeArena, domain: Option<u32>, first: f64) -> (SigId, SigId) {
        let mut b = SigBuilder::new(arena);
        let size = b.int(4);
        let mut head = b.real(first);
        if let Some(domain) = domain {
            let token = b.clock_env_token(domain);
            head = b.clocked(token, head);
        }
        let rest = [0.2, 0.3, 0.4].map(|v| b.real(v));
        let wave = b.waveform(&[head, rest[0], rest[1], rest[2]]);
        let generator = b.generate(wave);
        (b.wrtbl_readonly(size, generator), generator)
    }

    #[test]
    fn tables_differing_only_by_their_domain_share_one_owner() {
        let mut arena = TreeArena::new();
        let (t0, g0) = table(&mut arena, Some(0), 0.1);
        let (t1, g1) = table(&mut arena, Some(1), 0.1);
        let (top, g_top) = table(&mut arena, None, 0.1);
        assert_ne!(t0, t1, "the fixture must start from two table nodes");
        let mut keys = TableContentKeys::default();
        let f32_ty = FirType::Float32;
        assert_eq!(keys.owner(&arena, t0, 4, &f32_ty, g0), t0);
        assert_eq!(keys.owner(&arena, t1, 4, &f32_ty, g1), t0);
        assert_eq!(keys.owner(&arena, top, 4, &f32_ty, g_top), t0);
    }

    #[test]
    fn content_size_and_element_type_keep_tables_apart() {
        let mut arena = TreeArena::new();
        let (t0, g0) = table(&mut arena, Some(0), 0.1);
        let (other, g_other) = table(&mut arena, Some(1), 0.9);
        let mut keys = TableContentKeys::default();
        let f32_ty = FirType::Float32;
        assert_eq!(keys.owner(&arena, t0, 4, &f32_ty, g0), t0);
        assert_eq!(keys.owner(&arena, other, 4, &f32_ty, g_other), other);
        // The same generator at another size, or element type, is another
        // table.
        let resized = other;
        let mut keys = TableContentKeys::default();
        assert_eq!(keys.owner(&arena, t0, 4, &f32_ty, g0), t0);
        assert_eq!(keys.owner(&arena, resized, 8, &f32_ty, g0), resized);
        assert_eq!(
            keys.owner(&arena, resized, 4, &FirType::Float64, g0),
            resized
        );
    }
}
