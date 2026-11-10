//! References promoted to struct fields, after outlining (plan
//! `porting/fir-outline-reference-promotion-plan-2026-11-10-en.md`).
//!
//! A local of a DSP method that the method only declares and passes by
//! address to the functions outlined from it becomes a struct field: the
//! declaration becomes a store of its initial value at the same place, the
//! address arguments and the reference parameters go away, and the outlined
//! functions reach the field directly. Exact because a DSP method is not
//! re-entrant on one instance and the name is declared nowhere else (plan
//! §3). The output is returned only once [`super::promote_check`] has
//! accepted it.

use std::collections::{HashMap, HashSet, VecDeque};

use super::{Mutation, OutlineReport, block_items, is_scalar};
use crate::{AccessType, FirBuilder, FirId, FirMatch, FirStore, FirType, NamedType, match_fir};

/// One function of the module, decoded.
#[derive(Clone)]
struct Fun {
    name: String,
    typ: FirType,
    args: Vec<NamedType>,
    body: Option<FirId>,
    is_inline: bool,
}

/// The promoted copy of `module`, with the promoted names in order, or
/// `None` when nothing is promoted or a shape is outside the plan (the caller
/// keeps the outlined module).
pub(crate) fn promote_references(
    src: &FirStore,
    module: FirId,
    report: &OutlineReport,
    mutation: Mutation,
) -> Option<(FirStore, FirId, Vec<String>)> {
    let mut store = FirStore::new();
    let root = store.import_from(src, module);
    let FirMatch::Module {
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        sub_modules,
    } = match_fir(&store, root)
    else {
        return None;
    };
    let added: HashMap<&str, &super::OutlinedFunction> = report
        .outlined
        .iter()
        .map(|f| (f.name.as_str(), f))
        .collect();
    let items = block_items(&store, functions);
    let mut funs = Vec::with_capacity(items.len());
    for f in &items {
        let FirMatch::DeclareFun {
            name,
            typ,
            args,
            body,
            is_inline,
        } = match_fir(&store, *f)
        else {
            return None;
        };
        funs.push(Fun {
            name,
            typ,
            args,
            body,
            is_inline,
        });
    }

    // How many times each name is declared in the whole module.
    let declarations = declaration_counts(&store, root);

    // The promoted locals of each source method, and their types.
    let mut promoted: Vec<String> = Vec::new();
    let mut types: HashMap<String, FirType> = HashMap::new();
    for f in &funs {
        let Some(body) = f.body else { continue };
        if added.contains_key(f.name.as_str()) {
            continue;
        }
        let passed: HashSet<&str> = report
            .outlined
            .iter()
            .filter(|o| o.parent == f.name)
            .flat_map(|o| o.by_reference.iter().map(String::as_str))
            .collect();
        if passed.is_empty() {
            continue;
        }
        let top = block_items(&store, body);
        let other_uses = names_used_otherwise(&store, &top, &added);
        for item in top {
            let FirMatch::DeclareVar {
                name: x,
                typ,
                access: AccessType::Stack,
                init: Some(_),
            } = match_fir(&store, item)
            else {
                continue;
            };
            if !is_scalar(&typ) || !passed.contains(x.as_str()) {
                continue;
            }
            // This top-level declaration is the only one in the module.
            let declared_once = declarations.get(&x) == Some(&1);
            if declared_once && !other_uses.contains(&x) {
                types.insert(x.clone(), typ);
                promoted.push(x);
            }
        }
    }
    if promoted.is_empty() {
        return None;
    }
    let promoted_set: HashSet<String> = promoted.iter().cloned().collect();

    // Source methods first, then each outlined function with what its
    // caller removed (each is called once, from a function before it).
    let mut removed: HashMap<String, HashSet<String>> = HashMap::new();
    let mut new_bodies: HashMap<String, FirId> = HashMap::new();
    let mut queue: VecDeque<usize> = VecDeque::new();
    let mut mutation_done = false;
    for (i, f) in funs.iter().enumerate() {
        if added.contains_key(f.name.as_str()) {
            continue;
        }
        let Some(body) = f.body else { continue };
        let mut rw = Rewrite {
            store: &mut store,
            added: &added,
            refs: HashSet::new(),
            is_source: true,
            promoted: &promoted_set,
            removed: &mut removed,
            memo: HashMap::new(),
            failed: false,
        };
        let mut out = Vec::new();
        for item in block_items(rw.store, body) {
            match match_fir(rw.store, item) {
                FirMatch::DeclareVar {
                    name: x,
                    init: Some(init),
                    access: AccessType::Stack,
                    ..
                } if promoted_set.contains(&x) => {
                    if mutation == Mutation::PromoteKeepDeclaration && !mutation_done {
                        mutation_done = true;
                        out.push(item);
                    } else if mutation == Mutation::PromoteDropReset && !mutation_done {
                        mutation_done = true;
                    } else {
                        let init = rw.node(init);
                        out.push(FirBuilder::new(rw.store).store_var(x, AccessType::Struct, init));
                    }
                }
                _ => out.push(rw.node(item)),
            }
        }
        if rw.failed {
            return None;
        }
        let new_body = FirBuilder::new(&mut store).block(&out);
        new_bodies.insert(f.name.clone(), new_body);
        queue.extend((0..funs.len()).filter(|&j| {
            added
                .get(funs[j].name.as_str())
                .is_some_and(|o| o.parent == f.name)
        }));
        let _ = i;
    }
    let mut done: HashSet<usize> = HashSet::new();
    while let Some(j) = queue.pop_front() {
        if !done.insert(j) {
            continue;
        }
        let f = &funs[j];
        let body = f.body?;
        let refs = removed.get(&f.name).cloned().unwrap_or_default();
        let mut rw = Rewrite {
            store: &mut store,
            added: &added,
            refs,
            is_source: false,
            promoted: &promoted_set,
            removed: &mut removed,
            memo: HashMap::new(),
            failed: false,
        };
        let mut new_body = rw.node(body);
        if rw.failed {
            return None;
        }
        if mutation == Mutation::PromoteSwapFields
            && !mutation_done
            && let Some(refs) = removed.get(&f.name)
            && refs.len() >= 2
        {
            let mut names: Vec<&String> = refs.iter().collect();
            names.sort();
            let (a, b) = (names[0].clone(), names[1].clone());
            new_body = swap_fields(&mut store, new_body, &a, &b, &mut HashMap::new());
            mutation_done = true;
        }
        new_bodies.insert(f.name.clone(), new_body);
        queue.extend((0..funs.len()).filter(|&k| {
            added
                .get(funs[k].name.as_str())
                .is_some_and(|o| o.parent == f.name)
        }));
    }

    // The functions, with the reference parameters removed.
    let mut fields = block_items(&store, dsp_struct);
    let subs = block_items(&store, sub_modules);
    let mut b = FirBuilder::new(&mut store);
    let mut new_funs = Vec::with_capacity(funs.len());
    for f in &funs {
        let gone = removed.get(&f.name);
        let keep = |a: &NamedType| gone.is_none_or(|g| !g.contains(&a.name));
        let args: Vec<NamedType> = f.args.iter().filter(|a| keep(a)).cloned().collect();
        let typ = match &f.typ {
            FirType::Fun { ret, .. } if gone.is_some() => FirType::Fun {
                args: args.iter().map(|a| a.typ.clone()).collect(),
                ret: ret.clone(),
            },
            other => other.clone(),
        };
        let body = new_bodies.get(&f.name).copied().or(f.body);
        new_funs.push(b.declare_fun(f.name.clone(), typ, &args, body, f.is_inline));
    }
    let functions = b.block(&new_funs);
    for x in &promoted {
        fields.push(b.declare_var(x.clone(), types[x].clone(), AccessType::Struct, None));
    }
    let dsp_struct = b.block(&fields);
    let root = b.module(
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        &subs,
    );
    Some((store, root, promoted))
}

/// How many times each name is declared under `root`, a subtree reached
/// twice (hash-consing shares identical statements) counting twice. One walk:
/// a node already seen adds its declarations again only when it has any.
fn declaration_counts(store: &FirStore, root: FirId) -> HashMap<String, usize> {
    fn has_declaration(store: &FirStore, id: FirId, memo: &mut HashMap<FirId, bool>) -> bool {
        if let Some(&b) = memo.get(&id) {
            return b;
        }
        let own = matches!(
            match_fir(store, id),
            FirMatch::DeclareVar { .. } | FirMatch::DeclareTable { .. }
        );
        let b = own
            || store
                .arena
                .children(id)
                .unwrap_or(&[])
                .to_vec()
                .into_iter()
                .any(|c| has_declaration(store, c, memo));
        memo.insert(id, b);
        b
    }
    fn walk(
        store: &FirStore,
        id: FirId,
        seen: &mut HashSet<FirId>,
        has: &mut HashMap<FirId, bool>,
        out: &mut HashMap<String, usize>,
    ) {
        if !seen.insert(id) {
            if has_declaration(store, id, has) {
                // Counted again, in full, with a walk of its own.
                walk(store, id, &mut HashSet::new(), has, out);
            }
            return;
        }
        if let FirMatch::DeclareVar { name, .. } | FirMatch::DeclareTable { name, .. } =
            match_fir(store, id)
        {
            *out.entry(name).or_insert(0) += 1;
        }
        for &c in store.arena.children(id).unwrap_or(&[]) {
            walk(store, c, seen, has, out);
        }
    }
    let mut out = HashMap::new();
    walk(
        store,
        root,
        &mut HashSet::new(),
        &mut HashMap::new(),
        &mut out,
    );
    out
}

/// The names that occur in `top` (a method body's items) other than as
/// their top-level declaration or as an argument `LoadVarAddress(x)` of a
/// call to an outlined function: one walk for every candidate.
fn names_used_otherwise(
    store: &FirStore,
    top: &[FirId],
    added: &HashMap<&str, &super::OutlinedFunction>,
) -> HashSet<String> {
    fn walk(
        store: &FirStore,
        id: FirId,
        added: &HashMap<&str, &super::OutlinedFunction>,
        seen: &mut HashSet<FirId>,
        out: &mut HashSet<String>,
    ) {
        if !seen.insert(id) {
            return;
        }
        match match_fir(store, id) {
            FirMatch::Drop(value) => {
                if let FirMatch::FunCall { name, args, .. } = match_fir(store, value)
                    && added.contains_key(name.as_str())
                {
                    for a in args {
                        if !matches!(match_fir(store, a), FirMatch::LoadVarAddress { .. }) {
                            walk(store, a, added, seen, out);
                        }
                    }
                    return;
                }
            }
            FirMatch::DeclareVar { name, .. }
            | FirMatch::DeclareTable { name, .. }
            | FirMatch::LoadVar { name, .. }
            | FirMatch::StoreVar { name, .. }
            | FirMatch::LoadTable { name, .. }
            | FirMatch::StoreTable { name, .. }
            | FirMatch::TeeVar { name, .. }
            | FirMatch::ShiftArrayVar { name, .. }
            | FirMatch::LoadVarAddress { name, .. } => {
                out.insert(name);
            }
            _ => {}
        }
        for &c in store.arena.children(id).unwrap_or(&[]) {
            walk(store, c, added, seen, out);
        }
    }
    let mut out = HashSet::new();
    let mut seen = HashSet::new();
    for &item in top {
        match match_fir(store, item) {
            // A top-level declaration counts only through its initializer
            // (a second declaration of the name is caught by the counts).
            FirMatch::DeclareVar { init, .. } => {
                if let Some(init) = init {
                    walk(store, init, added, &mut seen, &mut out);
                }
            }
            _ => walk(store, item, added, &mut seen, &mut out),
        }
    }
    out
}

/// The rewrite of one function's body: `refs` are its removed reference
/// parameters (empty for a source method).
struct Rewrite<'a> {
    store: &'a mut FirStore,
    added: &'a HashMap<&'a str, &'a super::OutlinedFunction>,
    refs: HashSet<String>,
    /// A source method: its address arguments of promoted locals go.
    is_source: bool,
    promoted: &'a HashSet<String>,
    removed: &'a mut HashMap<String, HashSet<String>>,
    memo: HashMap<FirId, FirId>,
    failed: bool,
}

impl Rewrite<'_> {
    fn is_zero(&self, index: FirId) -> bool {
        matches!(
            match_fir(self.store, index),
            FirMatch::Int32 { value: 0, .. }
        )
    }

    fn node(&mut self, id: FirId) -> FirId {
        if let Some(&done) = self.memo.get(&id) {
            return done;
        }
        let out = match match_fir(self.store, id) {
            FirMatch::Drop(value) => match match_fir(self.store, value) {
                FirMatch::FunCall { name, args, typ } if self.added.contains_key(name.as_str()) => {
                    let mut kept = Vec::with_capacity(args.len());
                    let mut gone = HashSet::new();
                    for &a in &args {
                        let drop = match match_fir(self.store, a) {
                            FirMatch::LoadVarAddress { name: v, .. } => {
                                self.is_source && self.promoted.contains(&v)
                            }
                            FirMatch::LoadVar {
                                name: v,
                                access: AccessType::FunArgs,
                                ..
                            } => self.refs.contains(&v),
                            _ => false,
                        };
                        if drop {
                            let v = match match_fir(self.store, a) {
                                FirMatch::LoadVarAddress { name, .. }
                                | FirMatch::LoadVar { name, .. } => name,
                                _ => unreachable!(),
                            };
                            gone.insert(v);
                        } else {
                            kept.push(self.node(a));
                        }
                    }
                    // Called twice: outside the plan.
                    if !gone.is_empty() && self.removed.insert(name.clone(), gone).is_some() {
                        self.failed = true;
                    }
                    let mut b = FirBuilder::new(self.store);
                    let call = b.fun_call(name, &kept, typ);
                    b.drop_(call)
                }
                _ => self.children(id),
            },
            FirMatch::LoadTable {
                name,
                access: AccessType::FunArgs,
                index,
                typ,
            } if self.refs.contains(&name) => {
                if !self.is_zero(index) {
                    self.failed = true;
                }
                FirBuilder::new(self.store).load_var(name, AccessType::Struct, typ)
            }
            FirMatch::StoreTable {
                name,
                access: AccessType::FunArgs,
                index,
                value,
            } if self.refs.contains(&name) => {
                if !self.is_zero(index) {
                    self.failed = true;
                }
                let value = self.node(value);
                FirBuilder::new(self.store).store_var(name, AccessType::Struct, value)
            }
            FirMatch::LoadVar { name, .. }
            | FirMatch::StoreVar { name, .. }
            | FirMatch::LoadVarAddress { name, .. }
            | FirMatch::TeeVar { name, .. }
            | FirMatch::ShiftArrayVar { name, .. }
                if self.refs.contains(&name) =>
            {
                // A reference used other than at index 0 or passed on.
                self.failed = true;
                id
            }
            _ => self.children(id),
        };
        self.memo.insert(id, out);
        out
    }

    fn children(&mut self, id: FirId) -> FirId {
        let Some(children) = self.store.arena.children(id).map(<[FirId]>::to_vec) else {
            return id;
        };
        if children.is_empty() {
            return id;
        }
        let mut changed = false;
        let mut out = Vec::with_capacity(children.len());
        for c in children {
            let n = self.node(c);
            changed |= n != c;
            out.push(n);
        }
        match self.store.arena.kind(id).cloned() {
            Some(kind) if changed => self.store.arena.intern(kind, &out),
            _ => id,
        }
    }
}

/// The `PromoteSwapFields` mutation: struct accesses to `a` and `b` swapped.
fn swap_fields(
    store: &mut FirStore,
    id: FirId,
    a: &str,
    b: &str,
    memo: &mut HashMap<FirId, FirId>,
) -> FirId {
    if let Some(&done) = memo.get(&id) {
        return done;
    }
    let other = |n: &str| {
        if n == a {
            Some(b.to_owned())
        } else if n == b {
            Some(a.to_owned())
        } else {
            None
        }
    };
    let out = match match_fir(store, id) {
        FirMatch::LoadVar {
            name,
            access: AccessType::Struct,
            typ,
        } if other(&name).is_some() => {
            FirBuilder::new(store).load_var(other(&name).unwrap_or(name), AccessType::Struct, typ)
        }
        FirMatch::StoreVar {
            name,
            access: AccessType::Struct,
            value,
        } if other(&name).is_some() => {
            let value = swap_fields(store, value, a, b, memo);
            FirBuilder::new(store).store_var(
                other(&name).unwrap_or(name),
                AccessType::Struct,
                value,
            )
        }
        _ => {
            let Some(children) = store.arena.children(id).map(<[FirId]>::to_vec) else {
                return id;
            };
            let mut changed = false;
            let mut out = Vec::with_capacity(children.len());
            for c in children {
                let n = swap_fields(store, c, a, b, memo);
                changed |= n != c;
                out.push(n);
            }
            match store.arena.kind(id).cloned() {
                Some(kind) if changed => store.arena.intern(kind, &out),
                _ => id,
            }
        }
    };
    memo.insert(id, out);
    out
}
