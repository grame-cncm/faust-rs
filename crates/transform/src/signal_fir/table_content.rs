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
//! Equality is also modulo the names of the recursion variables a generator
//! binds. The prepared forest is in symbolic form, where every recursion group
//! has a variable of its own (`SYMREC(W1, body)`), and the two domain copies of
//! a stateful generator are two groups: `ba.time` in the generator of `os.osc`
//! is `W1` in one `ondemand` body and `W2` in the next. A generator is
//! compiled from its payload alone, so the name of a variable it binds cannot
//! reach the table. The key therefore renames those variables in order of
//! first visit. A reference to a group bound outside the generator keeps its
//! name: two such references name two different signals.
//!
//! The prepared forest itself is not rewritten. Dropping the annotations there
//! would also change the stateful nodes a generator shares with the body it is
//! written in, which renames and adds state in programs that have nothing to
//! share; keying the declaration leaves every program without a duplicated
//! table byte-identical.
//!
//! Writable tables (`rwtable`) are never keyed: each one is per-instance state
//! written by its own clocked write port.

use std::collections::{HashMap, HashSet};

use fir::FirType;
use signals::{SigId, SigMatch, match_sig};
use tlib::{NodeKind, TreeArena, match_sym_rec, match_sym_ref, sym_rec, sym_ref};

/// Equality of read-only table contents modulo clock annotations and the
/// names of bound recursion variables, one compiled module long.
///
/// The generator payloads are copied, without their `Clocked` wrappers and
/// with their bound recursion variables renamed in order of first visit, into
/// a private arena whose hash-consing then decides structural equality
/// exactly: no fingerprint, no collision to rule out.
#[derive(Default)]
pub(crate) struct TableContentKeys {
    /// Declocked copies of the generators seen so far, and their canonical
    /// forms.
    scratch: TreeArena,
    /// Source node to its declocked copy in `scratch`. Kept across tables:
    /// the copies of one waveform read in many domains share every element
    /// but the annotated first one.
    copies: HashMap<SigId, SigId>,
    /// Whether a recursion binder (`SYMREC(var, body)`) lies under a node of
    /// `scratch`. Kept across tables, like `copies`: a generator without one
    /// (a waveform) is its own canonical form and is not walked again.
    binders: HashMap<SigId, bool>,
    /// `(size, element type, canonical generator)` to the first table node
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
    /// generator modulo clock annotations and bound recursion variable names,
    /// or registers `table` as the owner of that content and returns it.
    /// `arena` is the arena `generator` lives in.
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
        let content = self.canonical_recursions(content);
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

    /// Whether a recursion binder lies under `id`, a node of the private
    /// arena.
    fn has_binder(&mut self, id: SigId) -> bool {
        if let Some(&known) = self.binders.get(&id) {
            return known;
        }
        let found = match_sym_rec(&self.scratch, id).is_some()
            || self
                .scratch
                .children(id)
                .map(<[SigId]>::to_vec)
                .unwrap_or_default()
                .into_iter()
                .any(|child| self.has_binder(child));
        self.binders.insert(id, found);
        found
    }

    /// `content`, a declocked copy, with the recursion variables it binds
    /// renamed `rec#0`, `rec#1`, ... in order of first visit (module doc).
    ///
    /// The renaming is injective and touches only variables bound under
    /// `content`, so two generators with one canonical form differ by the
    /// names of their own groups alone. Corresponding nodes of two such
    /// generators are visited in the same order: the walk is memoized by node,
    /// and hash-consing shares the same subtrees in both.
    fn canonical_recursions(&mut self, content: SigId) -> SigId {
        if !self.has_binder(content) {
            return content;
        }
        let mut bound = HashSet::new();
        self.bound_variables(content, &mut HashSet::new(), &mut bound);
        let mut walk = Renaming {
            bound,
            names: HashMap::new(),
            done: HashMap::new(),
        };
        self.rename(content, &mut walk)
    }

    /// The variables of the binders under `id`.
    fn bound_variables(&self, id: SigId, seen: &mut HashSet<SigId>, bound: &mut HashSet<SigId>) {
        if !seen.insert(id) {
            return;
        }
        if let Some((var, _)) = match_sym_rec(&self.scratch, id) {
            bound.insert(var);
        }
        for &child in self.scratch.children(id).unwrap_or(&[]) {
            self.bound_variables(child, seen, bound);
        }
    }

    fn rename(&mut self, id: SigId, walk: &mut Renaming) -> SigId {
        if let Some(&done) = walk.done.get(&id) {
            return done;
        }
        let out = if let Some((var, body)) = match_sym_rec(&self.scratch, id) {
            // The binder names its variable before its body is visited.
            let var = self.canonical_variable(var, walk);
            let body = self.rename(body, walk);
            sym_rec(&mut self.scratch, var, body)
        } else if let Some(var) = match_sym_ref(&self.scratch, id) {
            if walk.bound.contains(&var) {
                let var = self.canonical_variable(var, walk);
                sym_ref(&mut self.scratch, var)
            } else {
                id
            }
        } else {
            let children = self
                .scratch
                .children(id)
                .map(<[SigId]>::to_vec)
                .unwrap_or_default();
            let renamed: Vec<SigId> = children
                .iter()
                .map(|&child| self.rename(child, walk))
                .collect();
            match self.scratch.kind(id).cloned() {
                Some(kind) if renamed != children => self.scratch.intern(kind, &renamed),
                _ => id,
            }
        };
        walk.done.insert(id, out);
        out
    }

    /// The canonical variable of `var`, a bound variable: `rec#k`, `k` its
    /// rank of first visit. `#` is in no Faust identifier and in no variable
    /// `de_bruijn_to_sym` makes, so a canonical name is never a free one.
    fn canonical_variable(&mut self, var: SigId, walk: &mut Renaming) -> SigId {
        if let Some(&named) = walk.names.get(&var) {
            return named;
        }
        let named = self.scratch.symbol(format!("rec#{}", walk.names.len()));
        walk.names.insert(var, named);
        named
    }
}

/// One canonical renaming, of one generator.
struct Renaming {
    /// The variables the generator binds.
    bound: HashSet<SigId>,
    /// Bound variable to its canonical variable.
    names: HashMap<SigId, SigId>,
    /// Node to its renamed form.
    done: HashMap<SigId, SigId>,
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

    /// `Proj(0, SYMREC(var, [Proj(0, SYMREF(var))@domain + step]))`: a counter
    /// as the prepared forest holds it, its feedback annotated with `domain`
    /// when there is one.
    fn counter(arena: &mut TreeArena, var: &str, domain: Option<u32>, step: i32) -> SigId {
        let var = arena.symbol(var);
        let back = sym_ref(arena, var);
        let mut b = SigBuilder::new(arena);
        let mut feedback = b.proj(0, back);
        if let Some(domain) = domain {
            let token = b.clock_env_token(domain);
            feedback = b.clocked(token, feedback);
        }
        let step = b.int(step);
        let next = b.add(feedback, step);
        let nil = arena.nil();
        let body = arena.cons(next, nil);
        let group = sym_rec(arena, var, body);
        SigBuilder::new(arena).proj(0, group)
    }

    /// `WrTbl(4, Gen(content))`.
    fn table_of(arena: &mut TreeArena, content: SigId) -> (SigId, SigId) {
        let mut b = SigBuilder::new(arena);
        let size = b.int(4);
        let generator = b.generate(content);
        (b.wrtbl_readonly(size, generator), generator)
    }

    /// The owner of each table, registered in order, under fresh keys.
    fn owners(arena: &TreeArena, tables: &[(SigId, SigId)]) -> Vec<SigId> {
        let mut keys = TableContentKeys::default();
        tables
            .iter()
            .map(|&(table, generator)| keys.owner(arena, table, 4, &FirType::Int32, generator))
            .collect()
    }

    #[test]
    fn recursions_differing_by_their_variable_and_domain_share_one_owner() {
        // `ba.time` in the generator of `os.osc`, read in two `ondemand`
        // bodies and at the top level: three groups, three variables.
        let mut arena = TreeArena::new();
        let c0 = counter(&mut arena, "W1", Some(0), 1);
        let c1 = counter(&mut arena, "W2", Some(1), 1);
        let c_top = counter(&mut arena, "W7", None, 1);
        let tables = [c0, c1, c_top].map(|c| table_of(&mut arena, c));
        assert_ne!(tables[0].0, tables[1].0, "two table nodes to start from");
        assert_ne!(tables[1].0, tables[2].0, "two table nodes to start from");
        let t0 = tables[0].0;
        assert_eq!(owners(&arena, &tables), [t0, t0, t0]);
    }

    #[test]
    fn recursions_of_different_content_stay_apart() {
        let mut arena = TreeArena::new();
        let by_one = counter(&mut arena, "W1", Some(0), 1);
        let by_two = counter(&mut arena, "W2", Some(1), 2);
        let tables = [by_one, by_two].map(|c| table_of(&mut arena, c));
        assert_eq!(owners(&arena, &tables), [tables[0].0, tables[1].0]);
    }

    #[test]
    fn two_groups_are_named_in_order_of_first_visit() {
        let mut arena = TreeArena::new();
        // by 1 then by 2, under two namings: one content.
        let (a1, b2) = (
            counter(&mut arena, "W1", Some(0), 1),
            counter(&mut arena, "W2", Some(0), 2),
        );
        let first = SigBuilder::new(&mut arena).add(a1, b2);
        let (b1, a2) = (
            counter(&mut arena, "W4", Some(1), 1),
            counter(&mut arena, "W3", Some(1), 2),
        );
        let renamed = SigBuilder::new(&mut arena).add(b1, a2);
        // by 2 then by 1: another content.
        let (a2_first, b1_second) = (
            counter(&mut arena, "W5", Some(0), 2),
            counter(&mut arena, "W6", Some(0), 1),
        );
        let swapped = SigBuilder::new(&mut arena).add(a2_first, b1_second);
        let tables = [first, renamed, swapped].map(|c| table_of(&mut arena, c));
        let t0 = tables[0].0;
        assert_eq!(owners(&arena, &tables), [t0, t0, tables[2].0]);
    }

    /// `Proj(0, SYMREC(outer, [Proj(0, SYMREC(inner, [outer' + inner']))]))`.
    fn nested(arena: &mut TreeArena, outer: &str, inner: &str) -> SigId {
        let outer = arena.symbol(outer);
        let inner = arena.symbol(inner);
        let outer_back = sym_ref(arena, outer);
        let inner_back = sym_ref(arena, inner);
        let mut b = SigBuilder::new(arena);
        let x = b.proj(0, outer_back);
        let y = b.proj(0, inner_back);
        let sum = b.add(x, y);
        let nil = arena.nil();
        let inner_body = arena.cons(sum, nil);
        let inner_group = sym_rec(arena, inner, inner_body);
        let inner_out = SigBuilder::new(arena).proj(0, inner_group);
        let outer_body = arena.cons(inner_out, nil);
        let outer_group = sym_rec(arena, outer, outer_body);
        SigBuilder::new(arena).proj(0, outer_group)
    }

    #[test]
    fn a_group_inside_a_group_is_renamed_with_it() {
        let mut arena = TreeArena::new();
        let first = nested(&mut arena, "W1", "W2");
        let renamed = nested(&mut arena, "W8", "W3");
        let tables = [first, renamed].map(|c| table_of(&mut arena, c));
        assert_ne!(tables[0].0, tables[1].0, "two table nodes to start from");
        assert_eq!(owners(&arena, &tables), [tables[0].0, tables[0].0]);
    }

    #[test]
    fn a_reference_to_a_group_bound_outside_keeps_its_name() {
        let mut arena = TreeArena::new();
        // Two generators that read two different groups of the enclosing
        // program: the names are all that tells them apart, and they must.
        let free = |arena: &mut TreeArena, var: &str| {
            let var = arena.symbol(var);
            let back = sym_ref(arena, var);
            SigBuilder::new(arena).proj(0, back)
        };
        let f5 = free(&mut arena, "W5");
        let f6 = free(&mut arena, "W6");
        let tables = [f5, f6].map(|c| table_of(&mut arena, c));
        assert_eq!(owners(&arena, &tables), [tables[0].0, tables[1].0]);

        // The same beside a bound group: the bound name is forgotten, the
        // free one is not.
        let c1 = counter(&mut arena, "W1", Some(0), 1);
        let with_f5 = SigBuilder::new(&mut arena).add(c1, f5);
        let c2 = counter(&mut arena, "W2", Some(1), 1);
        let with_f6 = SigBuilder::new(&mut arena).add(c2, f6);
        let c3 = counter(&mut arena, "W3", Some(1), 1);
        let again_f5 = SigBuilder::new(&mut arena).add(c3, f5);
        let tables = [with_f5, with_f6, again_f5].map(|c| table_of(&mut arena, c));
        let t0 = tables[0].0;
        assert_eq!(owners(&arena, &tables), [t0, tables[1].0, t0]);
    }

    #[test]
    fn a_generator_without_recursion_is_its_own_canonical_form() {
        let mut arena = TreeArena::new();
        let (_, generator) = table(&mut arena, Some(0), 0.1);
        let mut keys = TableContentKeys::default();
        let copy = keys.declocked_copy(&arena, generator);
        assert_eq!(keys.canonical_recursions(copy), copy);
    }
}
