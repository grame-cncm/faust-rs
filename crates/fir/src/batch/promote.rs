//! Per-voice state in registers for the sample loop: the promotion pass of
//! the batch form.
//!
//! Plan: `porting/batch-state-register-promotion-plan-2026-11-08-en.md`
//! (§2 the pass, §3 the checker).
//!
//! In the batch contract every `Struct` field becomes arena memory, and the
//! generated `compute` reads and writes it at every sample. The Mojo compiler
//! does not keep those values in registers (an arena pointer may alias the
//! output buffers). [`promote_state`] rewrites `compute` so that it does not
//! need to: every `Struct` field accessed only as a scalar, or only at
//! `Int32` literal indices, is read once into a `Stack` local at the head of
//! the body, used through the local, and written back at the end of the body
//! when `compute` stores into it.
//!
//! It runs on the flattened module before [`super::layout_voices`], which
//! then lays out the entry loads and exit stores like any other access. It
//! does not depend on the layout: it is a scalar replacement of `compute`
//! state. Semantics are kept because a voice's state is touched by its own
//! `compute` call only, every access of a promoted element is replaced
//! uniformly, and the pass refuses the paths that could reach a field behind
//! its back: its address, a `TeeVar`, a shift, a call to a function of the
//! module, an early `Return`.
//!
//! # Public API mapping status
//! - `promote_state`, `PromotedState`, `PromotedElement`, `PromoteError`:
//!   no C++ counterpart (Rust-only, `adapted`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use super::arena_name;
use crate::{AccessType, FirBuilder, FirId, FirMatch, FirStore, FirType, match_fir};

/// The function the pass rewrites.
const COMPUTE: &str = "compute";

/// One promoted state element: a scalar field, or one element of an array
/// field.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PromotedElement {
    /// The `Struct` field.
    pub field: String,
    /// The element, `None` for a scalar field.
    pub index: Option<i32>,
    /// The `Stack` local holding it in `compute`.
    pub local: String,
    /// Whether `compute` stores into it (written back at the end).
    pub written: bool,
}

/// What [`promote_state`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromotedState {
    /// The promoted elements, in field declaration order then index.
    pub elements: Vec<PromotedElement>,
    /// `Struct` fields `compute` touches that stay in memory, with the reason.
    pub kept: Vec<(String, &'static str)>,
}

/// Why [`promote_state`] refused a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromoteError {
    /// The root is not a module.
    NotAModule,
    /// The output was rejected by the independent checker
    /// ([`super::promote_check`]); an internal error of the pass.
    Rejected(String),
}

impl fmt::Display for PromoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAModule => write!(f, "the root is not a FIR module"),
            Self::Rejected(why) => {
                write!(f, "the promotion was rejected by its checker: {why}")
            }
        }
    }
}

impl std::error::Error for PromoteError {}

/// Promotes the `compute` state of `module` in a copy of `src` (module doc).
/// Returns the new store, the new module root and what was promoted. A
/// module where nothing qualifies comes back with no element promoted.
///
/// The output is returned only once
/// [`super::promote_check::check_promotion`], written apart from the
/// producer, has accepted it (fail closed).
///
/// # Errors
/// [`PromoteError::NotAModule`], or [`PromoteError::Rejected`] if the checker
/// rejects the output.
pub fn promote_state(
    src: &FirStore,
    module: FirId,
) -> Result<(FirStore, FirId, PromotedState), PromoteError> {
    let (store, root, state) = promote_state_with(src, module, Mutation::None)?;
    super::promote_check::check_promotion(src, module, &store, root)
        .map_err(|v| PromoteError::Rejected(v.to_string()))?;
    Ok((store, root, state))
}

/// Deliberate defects of the producer, one per rejecting test of the checker
/// (plan §3). Only `None` is used outside the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum Mutation {
    None,
    /// The first written element is not stored back.
    DropWriteBack,
    /// The first read-only element is stored back.
    SpuriousWriteBack,
    /// The first body access to a promoted element is left on the field.
    LeaveOneAccess,
    /// The first array element's entry load reads the next element.
    WrongElement,
    /// The first field with a dynamic index is promoted anyway: its literal
    /// elements, and element 0.
    PromoteDynamic,
    /// The write-backs are placed before the body.
    ExitBeforeBody,
}

/// The producer without its checker.
pub(crate) fn promote_state_with(
    src: &FirStore,
    module: FirId,
    mutation: Mutation,
) -> Result<(FirStore, FirId, PromotedState), PromoteError> {
    tlib::on_compile_stack(|| promote_step(src, module, mutation))
}

fn block_items(store: &FirStore, id: FirId) -> Vec<FirId> {
    match match_fir(store, id) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    }
}

/// How `compute` uses one `Struct` field.
#[derive(Default)]
struct Use {
    /// Literal elements read or written (`None` for a scalar access).
    elements: BTreeSet<Option<i32>>,
    /// Literal elements stored into.
    stored: BTreeSet<Option<i32>>,
    /// Why the field cannot be promoted, if it cannot.
    blocked: Option<&'static str>,
}

impl Use {
    fn block(&mut self, why: &'static str) {
        self.blocked.get_or_insert(why);
    }
}

/// What a walk of `compute` found.
#[derive(Default)]
struct Scan {
    uses: BTreeMap<String, Use>,
    returns: bool,
    calls: BTreeSet<String>,
}

fn literal_index(store: &FirStore, index: FirId) -> Option<i32> {
    match match_fir(store, index) {
        FirMatch::Int32 { value, .. } => Some(value),
        _ => None,
    }
}

fn scan(store: &FirStore, id: FirId, seen: &mut HashSet<FirId>, out: &mut Scan) {
    if !seen.insert(id) {
        return;
    }
    match match_fir(store, id) {
        FirMatch::LoadVar {
            name,
            access: AccessType::Struct,
            ..
        } => {
            out.uses.entry(name).or_default().elements.insert(None);
        }
        FirMatch::StoreVar {
            name,
            access: AccessType::Struct,
            value,
        } => {
            let u = out.uses.entry(name).or_default();
            u.elements.insert(None);
            u.stored.insert(None);
            if store.value_type(value).is_none() {
                u.block("a stored value of unknown type");
            }
        }
        FirMatch::LoadTable {
            name,
            access: AccessType::Struct,
            index,
            ..
        } => {
            let at = literal_index(store, index);
            let u = out.uses.entry(name).or_default();
            match at {
                Some(k) => {
                    u.elements.insert(Some(k));
                }
                None => u.block("a dynamic index"),
            }
        }
        FirMatch::StoreTable {
            name,
            access: AccessType::Struct,
            index,
            value,
        } => {
            let at = literal_index(store, index);
            let u = out.uses.entry(name).or_default();
            match at {
                Some(k) => {
                    u.elements.insert(Some(k));
                    u.stored.insert(Some(k));
                }
                None => u.block("a dynamic index"),
            }
            if store.value_type(value).is_none() {
                u.block("a stored value of unknown type");
            }
        }
        FirMatch::LoadVarAddress {
            name,
            access: AccessType::Struct,
            ..
        } => out.uses.entry(name).or_default().block("its address"),
        FirMatch::TeeVar {
            name,
            access: AccessType::Struct,
            ..
        } => out
            .uses
            .entry(name)
            .or_default()
            .block("a store used as a value"),
        FirMatch::ShiftArrayVar {
            name,
            access: AccessType::Struct,
            ..
        } => out.uses.entry(name).or_default().block("an array shift"),
        FirMatch::Return(_) => out.returns = true,
        FirMatch::FunCall { name, .. } => {
            out.calls.insert(name);
        }
        _ => {}
    }
    if let Some(children) = store.arena.children(id) {
        for &child in children {
            scan(store, child, seen, out);
        }
    }
}

/// The `Struct` variables of the module: name, element type, length (1 for a
/// scalar), whether an array, in declaration order.
fn struct_vars(store: &FirStore, dsp_struct: FirId) -> Vec<(String, FirType, usize, bool)> {
    block_items(store, dsp_struct)
        .into_iter()
        .filter_map(|decl| match match_fir(store, decl) {
            FirMatch::DeclareVar {
                name,
                typ,
                access: AccessType::Struct,
                ..
            } => Some(match typ {
                FirType::Array(elem, n) => (name, *elem, n, true),
                other => (name, other, 1, false),
            }),
            _ => None,
        })
        .collect()
}

/// Every name declared anywhere under `id`, and the defined functions.
fn names_under(store: &FirStore, id: FirId, seen: &mut HashSet<FirId>, out: &mut HashSet<String>) {
    if !seen.insert(id) {
        return;
    }
    match match_fir(store, id) {
        FirMatch::DeclareVar { name, .. }
        | FirMatch::DeclareTable { name, .. }
        | FirMatch::DeclareFun { name, .. } => {
            out.insert(name);
        }
        FirMatch::ForLoop { var, .. } | FirMatch::SimpleForLoop { var, .. } => {
            out.insert(var);
        }
        _ => {}
    }
    if let Some(children) = store.arena.children(id) {
        for &child in children {
            names_under(store, child, seen, out);
        }
    }
}

fn local_name(field: &str, index: Option<i32>) -> String {
    match index {
        Some(k) => format!("{field}_reg{k}"),
        None => format!("{field}_reg"),
    }
}

fn promote_step(
    src: &FirStore,
    module: FirId,
    mutation: Mutation,
) -> Result<(FirStore, FirId, PromotedState), PromoteError> {
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
        return Err(PromoteError::NotAModule);
    };
    let function_items = block_items(&store, functions);
    let mut defined = HashSet::new();
    let mut compute = None;
    for &f in &function_items {
        if let FirMatch::DeclareFun {
            name,
            body: Some(body),
            ..
        } = match_fir(&store, f)
        {
            if name == COMPUTE {
                compute = Some((f, body));
            }
            defined.insert(name);
        }
    }
    let mut state = PromotedState::default();
    let Some((compute_fun, body)) = compute else {
        return Ok((store, root, state));
    };
    let mut found = Scan::default();
    scan(&store, body, &mut HashSet::new(), &mut found);
    if found.returns || found.calls.iter().any(|c| defined.contains(c)) {
        state.kept = found
            .uses
            .keys()
            .map(|f| {
                (
                    f.clone(),
                    if found.returns {
                        "compute returns early"
                    } else {
                        "compute calls a function of the module"
                    },
                )
            })
            .collect();
        return Ok((store, root, state));
    }
    let mut taken = HashSet::new();
    names_under(&store, root, &mut HashSet::new(), &mut taken);

    // Choose the promoted elements, in declaration order then index.
    let mut promoted: HashMap<(String, Option<i32>), (String, FirType)> = HashMap::new();
    let mut dynamic_mutated = false;
    for (field, elem, len, is_array) in struct_vars(&store, dsp_struct) {
        let Some(u) = found.uses.get_mut(&field) else {
            continue;
        };
        let mut why = u.blocked;
        if why == Some("a dynamic index")
            && mutation == Mutation::PromoteDynamic
            && !dynamic_mutated
        {
            // Its element 0 too, should the field have no literal access.
            dynamic_mutated = true;
            why = None;
            u.elements.insert(Some(0));
        }
        if why.is_none() && arena_name(&elem).is_none() {
            why = Some("a type no arena holds");
        }
        if why.is_none() {
            let shape_ok = u.elements.iter().all(|e| match e {
                None => !is_array,
                Some(k) => is_array && *k >= 0 && usize::try_from(*k).is_ok_and(|k| k < len),
            });
            if !shape_ok {
                why = Some("a whole-array or out-of-range access");
            }
        }
        if why.is_none()
            && u.elements
                .iter()
                .any(|e| taken.contains(&local_name(&field, *e)))
        {
            why = Some("its local name is taken");
        }
        if let Some(why) = why {
            state.kept.push((field, why));
            continue;
        }
        for &e in &u.elements {
            let local = local_name(&field, e);
            promoted.insert((field.clone(), e), (local.clone(), elem.clone()));
            state.elements.push(PromotedElement {
                field: field.clone(),
                index: e,
                local,
                written: u.stored.contains(&e),
            });
        }
    }
    if state.elements.is_empty() {
        return Ok((store, root, state));
    }

    let FirMatch::DeclareFun {
        name: fun_name,
        typ: fun_type,
        args: fun_args,
        is_inline,
        ..
    } = match_fir(&store, compute_fun)
    else {
        return Err(PromoteError::NotAModule);
    };
    let sub_module_items = block_items(&store, sub_modules);

    // The body with every access to a promoted element on its local.
    let mut rewriter = Rewriter {
        store: &mut store,
        promoted: &promoted,
        memo: HashMap::new(),
        mutation,
        left_one: false,
    };
    let new_body = rewriter.rewrite(body);
    let middle = block_items(&store, new_body);

    // Entry declarations and exit write-backs.
    let mut b = FirBuilder::new(&mut store);
    let mut entry = Vec::with_capacity(state.elements.len());
    let mut exit = Vec::new();
    let (mut wrong_done, mut drop_done, mut spurious_done) = (false, false, false);
    for e in &state.elements {
        let elem = promoted[&(e.field.clone(), e.index)].1.clone();
        let init = match e.index {
            None => b.load_var(&e.field, AccessType::Struct, elem.clone()),
            Some(k) => {
                let k = if mutation == Mutation::WrongElement && !wrong_done {
                    wrong_done = true;
                    k + 1
                } else {
                    k
                };
                let i = b.int32(k);
                b.load_table(&e.field, AccessType::Struct, i, elem.clone())
            }
        };
        entry.push(b.declare_var(&e.local, elem.clone(), AccessType::Stack, Some(init)));
        let write = if e.written {
            !(mutation == Mutation::DropWriteBack && !std::mem::replace(&mut drop_done, true))
        } else {
            mutation == Mutation::SpuriousWriteBack && !std::mem::replace(&mut spurious_done, true)
        };
        if write {
            let value = b.load_var(&e.local, AccessType::Stack, elem);
            exit.push(match e.index {
                None => b.store_var(&e.field, AccessType::Struct, value),
                Some(k) => {
                    let i = b.int32(k);
                    b.store_table(&e.field, AccessType::Struct, i, value)
                }
            });
        }
    }
    let mut items = entry;
    if mutation == Mutation::ExitBeforeBody {
        items.extend(exit);
        items.extend(middle);
    } else {
        items.extend(middle);
        items.extend(exit);
    }
    let new_body = b.block(&items);

    // The module with `compute` replaced.
    let new_fun = b.declare_fun(fun_name, fun_type, &fun_args, Some(new_body), is_inline);
    let new_functions: Vec<FirId> = function_items
        .iter()
        .map(|&f| if f == compute_fun { new_fun } else { f })
        .collect();
    let new_functions = b.block(&new_functions);
    let root = b.module(
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        new_functions,
        static_decls,
        &sub_module_items,
    );
    Ok((store, root, state))
}

/// Replaces the accesses to promoted elements by their locals.
struct Rewriter<'a> {
    store: &'a mut FirStore,
    promoted: &'a HashMap<(String, Option<i32>), (String, FirType)>,
    memo: HashMap<FirId, FirId>,
    mutation: Mutation,
    left_one: bool,
}

impl Rewriter<'_> {
    fn local(
        &self,
        name: &str,
        access: AccessType,
        index: Option<i32>,
    ) -> Option<(String, FirType)> {
        if access != AccessType::Struct {
            return None;
        }
        self.promoted.get(&(name.to_owned(), index)).cloned()
    }

    fn leave(&mut self) -> bool {
        if self.mutation == Mutation::LeaveOneAccess && !self.left_one {
            self.left_one = true;
            return true;
        }
        false
    }

    fn rewrite(&mut self, id: FirId) -> FirId {
        if let Some(&done) = self.memo.get(&id) {
            return done;
        }
        let out = match match_fir(self.store, id) {
            FirMatch::LoadVar { name, access, .. } => match self.local(&name, access, None) {
                Some(_) if self.leave() => id,
                Some((local, elem)) => {
                    FirBuilder::new(self.store).load_var(local, AccessType::Stack, elem)
                }
                None => id,
            },
            FirMatch::LoadTable {
                name,
                access,
                index,
                ..
            } => {
                let at = literal_index(self.store, index);
                match at.and_then(|k| self.local(&name, access, Some(k))) {
                    Some(_) if self.leave() => id,
                    Some((local, elem)) => {
                        FirBuilder::new(self.store).load_var(local, AccessType::Stack, elem)
                    }
                    None => self.rebuild(id),
                }
            }
            FirMatch::StoreVar {
                name,
                access,
                value,
            } => match self.local(&name, access, None) {
                Some((local, _)) => {
                    let value = self.rewrite(value);
                    FirBuilder::new(self.store).store_var(local, AccessType::Stack, value)
                }
                None => self.rebuild(id),
            },
            FirMatch::StoreTable {
                name,
                access,
                index,
                value,
            } => {
                let at = literal_index(self.store, index);
                match at.and_then(|k| self.local(&name, access, Some(k))) {
                    Some((local, _)) => {
                        let value = self.rewrite(value);
                        FirBuilder::new(self.store).store_var(local, AccessType::Stack, value)
                    }
                    None => self.rebuild(id),
                }
            }
            _ => self.rebuild(id),
        };
        self.memo.insert(id, out);
        out
    }

    /// `id` with its children rewritten, or `id` itself when none changes.
    fn rebuild(&mut self, id: FirId) -> FirId {
        let Some(children) = self.store.arena.children(id).map(<[FirId]>::to_vec) else {
            return id;
        };
        let mut changed = false;
        let mut rewritten = Vec::with_capacity(children.len());
        for child in children {
            let new = self.rewrite(child);
            changed |= new != child;
            rewritten.push(new);
        }
        if !changed {
            return id;
        }
        let Some(kind) = self.store.arena.kind(id).cloned() else {
            return id;
        };
        self.store.arena.intern(kind, &rewritten)
    }
}
