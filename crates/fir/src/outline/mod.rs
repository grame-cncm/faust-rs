//! Large blocks of a DSP method moved into methods of their own: the
//! outlining pass, for backends whose compiler is slow on large functions.
//!
//! Plan: `porting/mojo-large-program-outlining-plan-2026-11-09-en.md`
//! (§3 the pass, §4 the checker).
//!
//! Mojo 1.3 lowers one function in time much worse than linear in its size
//! (8,000 statements 19 s, 16,000 statements 491 s), and the `compute` of a
//! large program — 51 `ondemand` blocks in the sample loop of the coopiano
//! voice, 56,000 lines — does not build in hours. [`outline_large_blocks`]
//! rewrites a module so that no DSP method stays above a statement
//! budget when it can avoid it: while a method is too large, the largest
//! `then` or `else` block of one of its `If` statements becomes a new
//! function of the module, called from where the block was. The `If` stays in
//! the caller, so an `ondemand` block's function runs only when its clock
//! fires.
//!
//! The new function is a method: `dsp: Ptr(Obj)` first, then the block's free
//! local variables, sorted by name and named as in the caller. A variable the
//! block only reads is passed by value; one it stores into is passed by
//! reference, as `Ptr(T)` with `LoadVarAddress` at the call and index-0 table
//! accesses inside, which is exact C semantics. `Struct`, `Static` and
//! `Global` accesses are unchanged. A block that the pass cannot outline
//! exactly (a `Return`, a pointer or array local, an address taken, ...) stays
//! inline.
//!
//! A function still above [`OutlinePolicy::split_budget`] afterwards has its
//! dominant straight-line block cut into runs, each a function of its own;
//! a single-assignment local read after its run goes through a spill array
//! declared by the caller (`split.rs`, plan §3.4).
//!
//! A module whose methods are all under the budget comes back unchanged. The
//! output is returned only once [`check::check_outlining`], written apart
//! from the producer, has accepted it (fail closed).
//!
//! # Public API mapping status
//! - `outline_large_blocks`, `OutlinePolicy`, `OutlineReport`,
//!   `OutlinedFunction`, `RefusedBlock`, `OutlineError`: no C++ counterpart
//!   (Rust-only, `adapted`); C++ Faust does not outline.

#[cfg(test)]
mod tests;

pub mod check;
mod promote;
pub mod promote_check;
mod split;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;

use crate::{AccessType, FirBuilder, FirId, FirMatch, FirStore, FirType, NamedType, match_fir};

/// Statements above which a method is outlined ([`OutlinePolicy::budget`]).
pub const DEFAULT_BUDGET: usize = 2_000;
/// Smallest block outlined ([`OutlinePolicy::min_block`]).
pub const DEFAULT_MIN_BLOCK: usize = 64;
/// Statements above which a function is split into runs
/// ([`OutlinePolicy::split_budget`]).
pub const DEFAULT_SPLIT_BUDGET: usize = 8_000;
/// The DSP methods the pass rewrites, then the functions it creates.
const TARGET_FUNCTIONS: [&str; 5] = [
    "compute",
    "instanceConstants",
    "instanceResetUserInterface",
    "instanceClear",
    "staticInit",
];
/// The receiver argument of every DSP method, and of the new functions.
const DSP_ARG: &str = "dsp";
/// The infix of a new function's name: `<method>Outline<k>`.
const OUTLINE_INFIX: &str = "Outline";

/// When and what the pass outlines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutlinePolicy {
    /// A method with more statements than this (nested statements included,
    /// a call counting one) is outlined.
    pub budget: usize,
    /// A block with fewer statements is never outlined, and an outlining
    /// leaves at least this many statements in its method.
    pub min_block: usize,
    /// A function still above this after its blocks are outlined has its
    /// largest straight-line block cut into runs of at most `budget`
    /// statements, each moved into a function of its own (plan §3.4).
    pub split_budget: usize,
    /// Pass a free local of pointer type (`Ptr` of a scalar: the channel
    /// pointers `input0`, `output0` of `compute`) by value, so a block that
    /// reads or writes through it can be outlined. Exact C semantics, but
    /// Mojo cannot name the origin of a pointer parameter, so its backend
    /// leaves this off; the C++ backend turns it on (plan
    /// `porting/cpp-outlining-plan-2026-11-10-en.md`).
    pub pointer_params: bool,
    /// After outlining, turn a local of a DSP method that the method only
    /// declares and passes by address to its outlined functions into a
    /// struct field (`promote.rs`, plan
    /// `porting/fir-outline-reference-promotion-plan-2026-11-10-en.md`):
    /// the C++ compiler no longer carries thousands of address-taken locals
    /// across the sample loop. Off for Mojo; on for the C++ CLI.
    pub promote_references: bool,
}

impl Default for OutlinePolicy {
    fn default() -> Self {
        Self {
            budget: DEFAULT_BUDGET,
            min_block: DEFAULT_MIN_BLOCK,
            split_budget: DEFAULT_SPLIT_BUDGET,
            pointer_params: false,
            promote_references: false,
        }
    }
}

/// One function the pass created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlinedFunction {
    /// Its name.
    pub name: String,
    /// The function it was outlined from.
    pub parent: String,
    /// Statements of the block when it was outlined.
    pub statements: usize,
    /// Parameters passed by value, in parameter order.
    pub by_value: Vec<String>,
    /// Parameters passed by reference, in parameter order.
    pub by_reference: Vec<String>,
}

/// A candidate block the pass left inline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedBlock {
    /// The function holding it.
    pub function: String,
    /// Its statements.
    pub statements: usize,
    /// Why it cannot be outlined exactly.
    pub reason: &'static str,
}

/// What [`outline_large_blocks`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutlineReport {
    /// The created functions, in creation order (their order in the module).
    pub outlined: Vec<OutlinedFunction>,
    /// The refused candidates, each once.
    pub refused: Vec<RefusedBlock>,
    /// The locals promoted to struct fields
    /// ([`OutlinePolicy::promote_references`]), in struct order. They no
    /// longer appear in any `by_reference`.
    pub promoted: Vec<String>,
}

/// Why [`outline_large_blocks`] refused a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutlineError {
    /// The root is not a module.
    NotAModule,
    /// The output was rejected by the independent checker
    /// ([`check::check_outlining`] or [`promote_check::check_promotion`]);
    /// an internal error of the pass.
    Rejected(String),
}

impl fmt::Display for OutlineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAModule => write!(f, "the root is not a FIR module"),
            Self::Rejected(why) => write!(f, "the outlining was rejected by its checker: {why}"),
        }
    }
}

impl std::error::Error for OutlineError {}

/// Outlines the large blocks of the DSP methods of `module` in a copy of
/// `src` (module doc). Returns the new store, the new module root and what
/// was done.
///
/// # Errors
/// [`OutlineError::NotAModule`], or [`OutlineError::Rejected`] if the
/// checker rejects the output.
pub fn outline_large_blocks(
    src: &FirStore,
    module: FirId,
    policy: OutlinePolicy,
) -> Result<(FirStore, FirId, OutlineReport), OutlineError> {
    let (store, root, report) = outline_with(src, module, policy, Mutation::None)?;
    if !report.outlined.is_empty() {
        check::check_outlining(src, module, &store, root)
            .map_err(|v| OutlineError::Rejected(v.to_string()))?;
    }
    if policy.promote_references {
        return promote_checked(store, root, report, Mutation::None);
    }
    Ok((store, root, report))
}

/// The promotion of `root` (an outlined module) and its checker, or the
/// module unchanged when nothing is promoted.
pub(crate) fn promote_checked(
    store: FirStore,
    root: FirId,
    mut report: OutlineReport,
    mutation: Mutation,
) -> Result<(FirStore, FirId, OutlineReport), OutlineError> {
    if report.outlined.is_empty() {
        return Ok((store, root, report));
    }
    let Some((pstore, proot, promoted)) =
        tlib::on_compile_stack(|| promote::promote_references(&store, root, &report, mutation))
    else {
        return Ok((store, root, report));
    };
    promote_check::check_promotion(&store, root, &pstore, proot)
        .map_err(|v| OutlineError::Rejected(v.to_string()))?;
    let gone: HashSet<&String> = promoted.iter().collect();
    for f in &mut report.outlined {
        f.by_reference.retain(|n| !gone.contains(n));
    }
    report.promoted = promoted;
    Ok((pstore, proot, report))
}

/// Deliberate defects of the producer, one per rejecting test of the checker
/// (plan §4). Only `None` is used outside the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum Mutation {
    None,
    /// The first written variable is passed by value.
    ByValueWrite,
    /// The first free variable is left out of the parameters.
    MissingParam,
    /// The first two arguments after `dsp` are swapped at the call.
    SwapArgs,
    /// The last statement of the first outlined block is dropped.
    DropStatement,
    /// The first call statement is written twice.
    CallTwice,
    /// The first parameter is redeclared at the head of its function.
    ShadowParam,
    /// The first read of a reference is at index 1.
    WrongIndex,
    /// Locals that may not escape are spilled like the others.
    SpillMutable,
    /// The first read of a spill slot reads a neighbouring slot.
    SpillWrongSlot,
    /// The first two calls of a split block are swapped.
    SpillEarly,
    /// The first pointer passed by value is passed by address instead.
    PointerAsReference,
    /// The first pointer parameter is stored into at the head of its
    /// function.
    PointerStore,
    /// The first promoted local keeps its declaration: the field is never
    /// set.
    PromoteKeepDeclaration,
    /// The first promoted local's declaration is dropped, with no store.
    PromoteDropReset,
    /// Two promoted fields are swapped in the first function that received
    /// both.
    PromoteSwapFields,
}

/// The producer without its checker.
pub(crate) fn outline_with(
    src: &FirStore,
    module: FirId,
    policy: OutlinePolicy,
    mutation: Mutation,
) -> Result<(FirStore, FirId, OutlineReport), OutlineError> {
    tlib::on_compile_stack(|| outline_step(src, module, policy, mutation))
}

// ------------------------------------------------------------------ shared

pub(crate) fn block_items(store: &FirStore, id: FirId) -> Vec<FirId> {
    match match_fir(store, id) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    }
}

/// Every name declared anywhere under `id`.
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

fn is_scalar(typ: &FirType) -> bool {
    matches!(
        typ,
        FirType::Int32
            | FirType::Int64
            | FirType::Float32
            | FirType::Float64
            | FirType::FaustFloat
            | FirType::Bool
    )
}

fn is_local_access(access: AccessType) -> bool {
    matches!(
        access,
        AccessType::Stack | AccessType::Loop | AccessType::FunArgs
    )
}

fn dsp_type() -> FirType {
    FirType::Ptr(Box::new(FirType::Obj))
}

// --------------------------------------------------------------- the scope

/// What a visible name is, in the function being processed.
#[derive(Debug, Clone, PartialEq)]
enum Binding {
    /// A variable of this type.
    Var(FirType),
    /// A reference parameter of a function the pass created: `Ptr(T)`
    /// accessed at index 0.
    Ref(FirType),
}

/// The visible names, innermost scope last.
#[derive(Debug, Clone, Default)]
struct Env {
    scopes: Vec<HashMap<String, Binding>>,
}

impl Env {
    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: String, binding: Binding) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, binding);
        }
    }

    fn lookup(&self, name: &str) -> Option<&Binding> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }
}

// ------------------------------------------------------ the free variables

/// How a free variable is passed to a new function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Read only: by value.
    Value,
    /// A pointer local ([`OutlinePolicy::pointer_params`]): the pointer by
    /// value, its pointee read and written through it at any index.
    Pointer,
    /// Stored into: by reference, from the caller's variable's address.
    Ref,
    /// A reference of the caller, passed on.
    PassRef,
    /// A spill array of this many elements of the parameter's type, declared
    /// by the caller, by reference (plan §3.4).
    Spill(usize),
}

#[derive(Debug, Clone, PartialEq)]
struct Param {
    name: String,
    /// The variable's type (the referenced type for a reference).
    typ: FirType,
    mode: Mode,
    /// The caller's access to the variable.
    access: AccessType,
}

impl Param {
    fn fun_type(&self) -> FirType {
        match self.mode {
            Mode::Value | Mode::Pointer => self.typ.clone(),
            Mode::Ref | Mode::PassRef => FirType::Ptr(Box::new(self.typ.clone())),
            Mode::Spill(n) => FirType::Array(Box::new(self.typ.clone()), n),
        }
    }
}

/// How a block uses one free variable: which kinds of access it makes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FreeUse {
    access: AccessType,
    loads: bool,
    stores: bool,
    /// `LoadTable`/`StoreTable` at the literal index 0.
    at_zero: bool,
    /// `LoadTable`/`StoreTable` at another index.
    indexed: bool,
    /// Any other access: an address, a tee, a shift.
    other: bool,
}

/// The free variables of a block and how it uses them, sorted by name, or
/// why the block cannot be outlined whatever its context. A pure function of
/// the block's id: the pass memoizes it (`porting/MEMOIZATION.md` §2.31).
type Content = Result<BTreeMap<String, FreeUse>, &'static str>;

/// Walks a candidate block for its [`Content`].
struct Analysis<'a> {
    store: &'a FirStore,
    added: &'a HashSet<String>,
    inner: Vec<HashSet<String>>,
    declared: HashSet<String>,
    free: BTreeMap<String, FreeUse>,
}

/// The kinds of access [`Analysis::access`] tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Load,
    Store,
    AtZero,
    Indexed,
    Other,
}

impl Analysis<'_> {
    fn local(&self, name: &str) -> bool {
        self.inner.iter().any(|s| s.contains(name))
    }

    fn declare(&mut self, name: String) {
        self.declared.insert(name.clone());
        if let Some(scope) = self.inner.last_mut() {
            scope.insert(name);
        }
    }

    fn access(&mut self, name: &str, access: AccessType, kind: Kind) -> Result<(), &'static str> {
        if !is_local_access(access) || self.local(name) {
            return Ok(());
        }
        if name == DSP_ARG {
            return if access == AccessType::FunArgs && kind == Kind::Load {
                Ok(())
            } else {
                Err("an access to `dsp` other than a read")
            };
        }
        let entry = self.free.entry(name.to_owned()).or_insert(FreeUse {
            access,
            loads: false,
            stores: false,
            at_zero: false,
            indexed: false,
            other: false,
        });
        if entry.access != access {
            return Err("a free variable accessed two ways");
        }
        match kind {
            Kind::Load => entry.loads = true,
            Kind::Store => entry.stores = true,
            Kind::AtZero => entry.at_zero = true,
            Kind::Indexed => entry.indexed = true,
            Kind::Other => entry.other = true,
        }
        Ok(())
    }

    fn block(&mut self, block: FirId) -> Result<(), &'static str> {
        self.inner.push(HashSet::new());
        for item in block_items(self.store, block) {
            self.node(item)?;
        }
        self.inner.pop();
        Ok(())
    }

    fn node(&mut self, id: FirId) -> Result<(), &'static str> {
        match match_fir(self.store, id) {
            FirMatch::Block(_) => return self.block(id),
            FirMatch::DeclareVar { name, init, .. } => {
                if let Some(init) = init {
                    self.node(init)?;
                }
                self.declare(name);
                return Ok(());
            }
            FirMatch::DeclareTable { name, values, .. } => {
                for v in values {
                    self.node(v)?;
                }
                self.declare(name);
                return Ok(());
            }
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                self.node(cond)?;
                self.block(then_block)?;
                if let Some(b) = else_block {
                    self.block(b)?;
                }
                return Ok(());
            }
            FirMatch::Control { cond, stmt } => {
                self.node(cond)?;
                self.inner.push(HashSet::new());
                self.node(stmt)?;
                self.inner.pop();
                return Ok(());
            }
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => {
                self.node(cond)?;
                for (_, b) in cases {
                    self.block(b)?;
                }
                if let Some(b) = default {
                    self.block(b)?;
                }
                return Ok(());
            }
            FirMatch::ForLoop {
                init,
                end,
                step,
                body,
                ..
            } => {
                self.inner.push(HashSet::new());
                self.node(init)?;
                self.node(end)?;
                self.node(step)?;
                self.block(body)?;
                self.inner.pop();
                return Ok(());
            }
            FirMatch::SimpleForLoop {
                var, upper, body, ..
            } => {
                self.node(upper)?;
                self.inner.push(HashSet::new());
                self.declare(var);
                self.block(body)?;
                self.inner.pop();
                return Ok(());
            }
            FirMatch::IteratorForLoop { .. } => return Err("an iterator loop"),
            FirMatch::WhileLoop { cond, body } => {
                self.node(cond)?;
                return self.block(body);
            }
            FirMatch::Return(_) => return Err("a return"),
            FirMatch::LoadVar { name, access, .. } => self.access(&name, access, Kind::Load)?,
            FirMatch::StoreVar { name, access, .. } => self.access(&name, access, Kind::Store)?,
            FirMatch::LoadTable {
                name,
                access,
                index,
                ..
            }
            | FirMatch::StoreTable {
                name,
                access,
                index,
                ..
            } => {
                let at_zero = matches!(
                    match_fir(self.store, index),
                    FirMatch::Int32 { value: 0, .. }
                );
                let kind = if at_zero { Kind::AtZero } else { Kind::Indexed };
                self.access(&name, access, kind)?;
            }
            FirMatch::LoadVarAddress { name, access, .. }
            | FirMatch::TeeVar { name, access, .. }
            | FirMatch::ShiftArrayVar { name, access, .. } => {
                self.access(&name, access, Kind::Other)?;
            }
            FirMatch::FunCall { name, .. } if self.added.contains(&name) => {
                return Err("a call to an outlined function");
            }
            _ => {}
        }
        if let Some(children) = self.store.arena.children(id) {
            for &child in children {
                self.node(child)?;
            }
        }
        Ok(())
    }
}

/// The [`Content`] of `block`.
fn content(store: &FirStore, added: &HashSet<String>, block: FirId) -> Content {
    content_items(store, added, &block_items(store, block))
}

/// The [`Content`] of a run of statements, analysed as one block.
fn content_items(store: &FirStore, added: &HashSet<String>, items: &[FirId]) -> Content {
    let mut a = Analysis {
        store,
        added,
        inner: vec![HashSet::new()],
        declared: HashSet::new(),
        free: BTreeMap::new(),
    };
    for &item in items {
        a.node(item)?;
    }
    if a.free.keys().any(|n| a.declared.contains(n)) {
        return Err("a declaration of a name the block also reads from outside");
    }
    Ok(a.free)
}

/// The parameters of a block of content `uses` at a position where `env` is
/// visible, or why it cannot be outlined there. `pointers` is
/// [`OutlinePolicy::pointer_params`].
fn resolve(
    env: &Env,
    uses: &BTreeMap<String, FreeUse>,
    pointers: bool,
) -> Result<Vec<Param>, &'static str> {
    let mut params = Vec::with_capacity(uses.len());
    for (name, u) in uses {
        let (typ, mode) = match env.lookup(name) {
            None => return Err("an undeclared free variable"),
            Some(Binding::Var(typ @ FirType::Ptr(inner))) if pointers && is_scalar(inner) => {
                if u.stores || u.other {
                    return Err("a store, address, tee or shift of a free pointer");
                }
                (typ.clone(), Mode::Pointer)
            }
            Some(Binding::Var(typ)) => {
                if u.at_zero || u.indexed || u.other {
                    return Err("an address, indexed access, tee or shift of a free variable");
                }
                if !is_scalar(typ) {
                    return Err("a free variable of pointer, array or vector type");
                }
                (typ.clone(), if u.stores { Mode::Ref } else { Mode::Value })
            }
            Some(Binding::Ref(typ)) => {
                if u.loads || u.stores || u.indexed || u.other {
                    return Err("a reference used other than at index 0");
                }
                (typ.clone(), Mode::PassRef)
            }
        };
        params.push(Param {
            name: name.clone(),
            typ,
            mode,
            access: u.access,
        });
    }
    Ok(params)
}

// ------------------------------------------------------- the candidates

struct Candidate {
    ordinal: usize,
    size: usize,
    block: FirId,
    /// Pre-order positions of the `then`/`else` blocks enclosing it.
    ancestors: Vec<usize>,
    analysis: Result<Vec<Param>, &'static str>,
}

/// One function the pass created, before its body is final.
struct Added {
    name: String,
    root: String,
    parent: String,
    params: Vec<Param>,
    body: FirId,
    statements: usize,
    /// A run of phase 2: not split again (plan §3.4).
    is_run: bool,
}

struct Pass {
    store: FirStore,
    policy: OutlinePolicy,
    mutation: Mutation,
    mutated: bool,
    taken: HashSet<String>,
    added_names: HashSet<String>,
    next: usize,
    queue: VecDeque<usize>,
    added: Vec<Added>,
    report: OutlineReport,
    refused_seen: HashSet<(String, FirId)>,
    contents: HashMap<FirId, Content>,
    next_spill: usize,
}

impl Pass {
    /// `true` once, for the first site of `m`.
    fn mutate(&mut self, m: Mutation) -> bool {
        if self.mutation == m && !self.mutated {
            self.mutated = true;
            return true;
        }
        false
    }

    fn fresh_name(&mut self, root: &str) -> String {
        loop {
            let name = format!("{root}{OUTLINE_INFIX}{}", self.next);
            self.next += 1;
            if !self.taken.contains(&name) {
                self.taken.insert(name.clone());
                self.added_names.insert(name.clone());
                return name;
            }
        }
    }

    /// Whether `block` is only a call to a function the pass created: a block
    /// already outlined, not a candidate.
    fn is_call_block(&self, block: FirId) -> bool {
        let items = block_items(&self.store, block);
        let [stmt] = items.as_slice() else {
            return false;
        };
        let FirMatch::Drop(call) = match_fir(&self.store, *stmt) else {
            return false;
        };
        matches!(match_fir(&self.store, call), FirMatch::FunCall { name, .. } if self.added_names.contains(&name))
    }

    /// The memoized [`Content`] of `block`.
    fn content_of(&mut self, block: FirId) -> Content {
        if let Some(c) = self.contents.get(&block) {
            return c.clone();
        }
        let c = content(&self.store, &self.added_names, block);
        self.contents.insert(block, c.clone());
        c
    }

    /// Walks a block: records its candidates (every `then`/`else` block of at
    /// least `min_block` statements, numbered in pre-order) and returns its
    /// size: its statements, nested ones included (a nested block counts its
    /// items, a call counts one).
    fn collect_block(
        &mut self,
        block: FirId,
        env: &mut Env,
        ord: &mut usize,
        path: &mut Vec<usize>,
        out: &mut Vec<Candidate>,
    ) -> usize {
        env.push();
        let mut size = 0;
        for item in block_items(&self.store, block) {
            size += self.collect_stmt(item, env, ord, path, out);
        }
        env.pop();
        size
    }

    fn collect_stmt(
        &mut self,
        stmt: FirId,
        env: &mut Env,
        ord: &mut usize,
        path: &mut Vec<usize>,
        out: &mut Vec<Candidate>,
    ) -> usize {
        match match_fir(&self.store, stmt) {
            FirMatch::Block(_) => return self.collect_block(stmt, env, ord, path, out),
            FirMatch::DeclareVar { name, typ, .. } => env.declare(name, Binding::Var(typ)),
            FirMatch::DeclareTable {
                name,
                elem_type,
                values,
                ..
            } => env.declare(
                name,
                Binding::Var(FirType::Array(Box::new(elem_type), values.len())),
            ),
            FirMatch::If {
                then_block,
                else_block,
                ..
            } => {
                let mut size = 1;
                for b in std::iter::once(then_block).chain(else_block) {
                    let ordinal = *ord;
                    *ord += 1;
                    path.push(ordinal);
                    let bsize = self.collect_block(b, env, ord, path, out);
                    path.pop();
                    size += bsize;
                    if bsize >= self.policy.min_block.max(1) && !self.is_call_block(b) {
                        let pointers = self.policy.pointer_params;
                        let analysis = self
                            .content_of(b)
                            .and_then(|uses| resolve(env, &uses, pointers));
                        out.push(Candidate {
                            ordinal,
                            size: bsize,
                            block: b,
                            ancestors: path.clone(),
                            analysis,
                        });
                    }
                }
                return size;
            }
            FirMatch::Control { stmt, .. } => {
                env.push();
                let size = self.collect_stmt(stmt, env, ord, path, out);
                env.pop();
                return 1 + size;
            }
            FirMatch::Switch { cases, default, .. } => {
                let mut size = 1;
                for (_, b) in cases {
                    size += self.collect_block(b, env, ord, path, out);
                }
                if let Some(b) = default {
                    size += self.collect_block(b, env, ord, path, out);
                }
                return size;
            }
            FirMatch::ForLoop { init, body, .. } => {
                env.push();
                // The loop variable's declaration is part of the loop.
                let _ = self.collect_stmt(init, env, ord, path, out);
                let size = self.collect_block(body, env, ord, path, out);
                env.pop();
                return 1 + size;
            }
            FirMatch::SimpleForLoop {
                var, upper, body, ..
            } => {
                let typ = self.store.value_type(upper).unwrap_or(FirType::Int32);
                env.push();
                env.declare(var, Binding::Var(typ));
                let size = self.collect_block(body, env, ord, path, out);
                env.pop();
                return 1 + size;
            }
            FirMatch::IteratorForLoop { body, .. } | FirMatch::WhileLoop { body, .. } => {
                return 1 + self.collect_block(body, env, ord, path, out);
            }
            _ => {}
        }
        1
    }

    /// Outlines the large blocks of one function until it fits: the
    /// candidates are taken largest first (ties in pre-order), skipping the
    /// ones refused, nested in a block already taken, or leaving fewer than
    /// `min_block` statements; each taken block shrinks the function by its
    /// size less one (the call).
    fn process(&mut self, fname: &str, root: &str, base: &Env, body: FirId, split: bool) -> FirId {
        let body = self.outline_blocks(fname, root, base, body);
        if split {
            self.split_large(fname, root, base, body)
        } else {
            body
        }
    }

    /// Phase 1 (plan §3.1): the large `then`/`else` blocks of one function.
    fn outline_blocks(&mut self, fname: &str, root: &str, base: &Env, body: FirId) -> FirId {
        let mut env = base.clone();
        let mut candidates = Vec::new();
        let fsize = self.collect_block(body, &mut env, &mut 0, &mut Vec::new(), &mut candidates);
        if fsize <= self.policy.budget {
            return body;
        }
        candidates.sort_by(|a, b| b.size.cmp(&a.size).then(a.ordinal.cmp(&b.ordinal)));
        let mut current = fsize;
        let mut taken = HashSet::new();
        let mut targets = HashMap::new();
        for c in candidates {
            if current <= self.policy.budget {
                break;
            }
            if c.size + self.policy.min_block > current
                || c.ancestors.iter().any(|a| taken.contains(a))
            {
                continue;
            }
            let mut params = match c.analysis {
                Ok(params) => params,
                Err(reason) => {
                    if self.refused_seen.insert((fname.to_owned(), c.block)) {
                        self.report.refused.push(RefusedBlock {
                            function: fname.to_owned(),
                            statements: c.size,
                            reason,
                        });
                    }
                    continue;
                }
            };
            taken.insert(c.ordinal);
            current -= c.size - 1;
            let name = self.fresh_name(root);
            if !params.is_empty() && self.mutate(Mutation::MissingParam) {
                params.remove(0);
            }
            if params.iter().any(|p| p.mode == Mode::Ref)
                && self.mutate(Mutation::ByValueWrite)
                && let Some(p) = params.iter_mut().find(|p| p.mode == Mode::Ref)
            {
                p.mode = Mode::Value;
            }
            let callee = self.callee_body(c.block, &params);
            let call = self.call_stmt(&name, &params);
            let replacement = if self.mutate(Mutation::CallTwice) {
                FirBuilder::new(&mut self.store).block(&[call, call])
            } else {
                FirBuilder::new(&mut self.store).block(&[call])
            };
            targets.insert(c.ordinal, replacement);
            self.queue.push_back(self.added.len());
            self.added.push(Added {
                name,
                root: root.to_owned(),
                parent: fname.to_owned(),
                params,
                body: callee,
                statements: c.size,
                is_run: false,
            });
        }
        if targets.is_empty() {
            return body;
        }
        self.replace_block(body, &targets, &mut 0)
    }

    /// The body of the new function: the block with its free variables
    /// turned into parameter accesses.
    fn callee_body(&mut self, block: FirId, params: &[Param]) -> FirId {
        let modes: HashMap<String, (Mode, FirType)> = params
            .iter()
            .map(|p| (p.name.clone(), (p.mode, p.typ.clone())))
            .collect();
        let mut rw = Rewriter {
            store: &mut self.store,
            modes: &modes,
            memo: HashMap::new(),
            wrong_index: self.mutation == Mutation::WrongIndex && !self.mutated,
            wrong_index_done: false,
        };
        let mut out = rw.rewrite(block);
        if rw.wrong_index_done {
            self.mutated = true;
        }
        let mut items = block_items(&self.store, out);
        if self.mutate(Mutation::DropStatement) {
            items.pop();
            out = FirBuilder::new(&mut self.store).block(&items);
        }
        if let Some(p) = params.iter().find(|p| p.mode == Mode::Pointer)
            && self.mutate(Mutation::PointerStore)
        {
            let mut b = FirBuilder::new(&mut self.store);
            let same = b.load_var(&p.name, AccessType::FunArgs, p.typ.clone());
            let store = b.store_var(&p.name, AccessType::FunArgs, same);
            items.insert(0, store);
            out = b.block(&items);
        }
        if let Some(first) = params.first()
            && self.mutate(Mutation::ShadowParam)
        {
            let mut b = FirBuilder::new(&mut self.store);
            let decl = b.declare_var(&first.name, first.typ.clone(), AccessType::Stack, None);
            items.insert(0, decl);
            out = b.block(&items);
        }
        out
    }

    fn call_stmt(&mut self, name: &str, params: &[Param]) -> FirId {
        let by_address = params
            .iter()
            .position(|p| p.mode == Mode::Pointer)
            .filter(|_| self.mutate(Mutation::PointerAsReference));
        let mut b = FirBuilder::new(&mut self.store);
        let mut args = vec![b.load_var(DSP_ARG, AccessType::FunArgs, dsp_type())];
        for (i, p) in params.iter().enumerate() {
            args.push(match p.mode {
                Mode::Pointer if by_address == Some(i) => {
                    b.load_var_address(&p.name, p.access, p.typ.clone())
                }
                Mode::Value | Mode::Pointer => b.load_var(&p.name, p.access, p.typ.clone()),
                Mode::Ref => b.load_var_address(&p.name, p.access, p.fun_type()),
                Mode::PassRef => b.load_var(&p.name, AccessType::FunArgs, p.fun_type()),
                Mode::Spill(_) => b.load_var(&p.name, AccessType::Stack, p.fun_type()),
            });
        }
        if args.len() >= 3 && self.mutation == Mutation::SwapArgs && !self.mutated {
            self.mutated = true;
            args.swap(1, 2);
        }
        let call = b.fun_call(name, &args, FirType::Void);
        b.drop_(call)
    }

    /// `block` with each `then`/`else` block whose pre-order position is a
    /// key of `targets` replaced by its value. The walk numbers the blocks as
    /// [`Self::collect_block`] does, those inside a replaced block included.
    fn replace_block(
        &mut self,
        block: FirId,
        targets: &HashMap<usize, FirId>,
        ord: &mut usize,
    ) -> FirId {
        let items = block_items(&self.store, block);
        let mut changed = false;
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let new = self.replace_stmt(item, targets, ord);
            changed |= new != item;
            out.push(new);
        }
        if changed {
            FirBuilder::new(&mut self.store).block(&out)
        } else {
            block
        }
    }

    fn replace_stmt(
        &mut self,
        stmt: FirId,
        targets: &HashMap<usize, FirId>,
        ord: &mut usize,
    ) -> FirId {
        match match_fir(&self.store, stmt) {
            FirMatch::Block(_) => self.replace_block(stmt, targets, ord),
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                let mut sub = |pass: &mut Self, b: FirId| {
                    let k = *ord;
                    *ord += 1;
                    let new = pass.replace_block(b, targets, ord);
                    targets.get(&k).copied().unwrap_or(new)
                };
                let new_then = sub(self, then_block);
                let new_else = else_block.map(|b| sub(self, b));
                if new_then == then_block && new_else == else_block {
                    stmt
                } else {
                    FirBuilder::new(&mut self.store).if_(cond, new_then, new_else)
                }
            }
            FirMatch::Control { cond, stmt: inner } => {
                let new = self.replace_stmt(inner, targets, ord);
                if new == inner {
                    stmt
                } else {
                    FirBuilder::new(&mut self.store).control(cond, new)
                }
            }
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => {
                let mut changed = false;
                let mut new_cases = Vec::with_capacity(cases.len());
                for (v, b) in cases {
                    let new = self.replace_block(b, targets, ord);
                    changed |= new != b;
                    new_cases.push((v, new));
                }
                let new_default = default.map(|b| self.replace_block(b, targets, ord));
                changed |= new_default != default;
                if changed {
                    FirBuilder::new(&mut self.store).switch(cond, &new_cases, new_default)
                } else {
                    stmt
                }
            }
            FirMatch::ForLoop {
                var,
                init,
                end,
                step,
                body,
                is_reverse,
            } => {
                let new = self.replace_block(body, targets, ord);
                if new == body {
                    stmt
                } else {
                    FirBuilder::new(&mut self.store).for_loop(var, init, end, step, new, is_reverse)
                }
            }
            FirMatch::SimpleForLoop {
                var,
                upper,
                body,
                is_reverse,
            } => {
                let new = self.replace_block(body, targets, ord);
                if new == body {
                    stmt
                } else {
                    FirBuilder::new(&mut self.store).simple_for_loop(var, upper, new, is_reverse)
                }
            }
            FirMatch::IteratorForLoop {
                iterators,
                is_reverse,
                body,
            } => {
                let new = self.replace_block(body, targets, ord);
                if new == body {
                    stmt
                } else {
                    let names: Vec<&str> = iterators.iter().map(String::as_str).collect();
                    FirBuilder::new(&mut self.store).iterator_for_loop(&names, is_reverse, new)
                }
            }
            FirMatch::WhileLoop { cond, body } => {
                let new = self.replace_block(body, targets, ord);
                if new == body {
                    stmt
                } else {
                    FirBuilder::new(&mut self.store).while_loop(cond, new)
                }
            }
            _ => stmt,
        }
    }
}

/// Turns the free-variable accesses of a block into parameter accesses.
/// The block declares none of the parameter names (the analysis refuses it
/// otherwise), so the rewrite goes by name.
struct Rewriter<'a> {
    store: &'a mut FirStore,
    modes: &'a HashMap<String, (Mode, FirType)>,
    memo: HashMap<FirId, FirId>,
    wrong_index: bool,
    wrong_index_done: bool,
}

impl Rewriter<'_> {
    fn mode(&self, name: &str, access: AccessType) -> Option<Mode> {
        if !is_local_access(access) {
            return None;
        }
        self.modes.get(name).map(|(m, _)| *m)
    }

    fn zero(&mut self) -> FirId {
        let index = if self.wrong_index && !self.wrong_index_done {
            self.wrong_index_done = true;
            1
        } else {
            0
        };
        FirBuilder::new(self.store).int32(index)
    }

    fn rewrite(&mut self, id: FirId) -> FirId {
        if let Some(&done) = self.memo.get(&id) {
            return done;
        }
        let out = match match_fir(self.store, id) {
            FirMatch::LoadVar { name, access, typ } => match self.mode(&name, access) {
                Some(Mode::Value | Mode::Pointer) => {
                    FirBuilder::new(self.store).load_var(name, AccessType::FunArgs, typ)
                }
                Some(Mode::Ref) => {
                    let zero = self.zero();
                    FirBuilder::new(self.store).load_table(name, AccessType::FunArgs, zero, typ)
                }
                _ => id,
            },
            FirMatch::StoreVar {
                name,
                access,
                value,
            } => match self.mode(&name, access) {
                Some(Mode::Ref) => {
                    let value = self.rewrite(value);
                    let zero = FirBuilder::new(self.store).int32(0);
                    FirBuilder::new(self.store).store_table(name, AccessType::FunArgs, zero, value)
                }
                // A written variable passed by value (the `ByValueWrite`
                // mutation only: the analysis passes it by reference).
                Some(Mode::Value) => {
                    let value = self.rewrite(value);
                    FirBuilder::new(self.store).store_var(name, AccessType::FunArgs, value)
                }
                _ => self.rebuild(id),
            },
            // The pointee of a pointer passed by value, through the parameter.
            FirMatch::LoadTable {
                name,
                access,
                index,
                typ,
            } if self.mode(&name, access) == Some(Mode::Pointer) => {
                let index = self.rewrite(index);
                FirBuilder::new(self.store).load_table(name, AccessType::FunArgs, index, typ)
            }
            FirMatch::StoreTable {
                name,
                access,
                index,
                value,
            } if self.mode(&name, access) == Some(Mode::Pointer) => {
                let index = self.rewrite(index);
                let value = self.rewrite(value);
                FirBuilder::new(self.store).store_table(name, AccessType::FunArgs, index, value)
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
        if children.is_empty() {
            return id;
        }
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

fn outline_step(
    src: &FirStore,
    module: FirId,
    policy: OutlinePolicy,
    mutation: Mutation,
) -> Result<(FirStore, FirId, OutlineReport), OutlineError> {
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
        return Err(OutlineError::NotAModule);
    };
    let mut taken = HashSet::new();
    names_under(&store, root, &mut HashSet::new(), &mut taken);
    let mut pass = Pass {
        store,
        policy,
        mutation,
        mutated: false,
        taken,
        added_names: HashSet::new(),
        next: 0,
        queue: VecDeque::new(),
        added: Vec::new(),
        report: OutlineReport::default(),
        refused_seen: HashSet::new(),
        contents: HashMap::new(),
        next_spill: 0,
    };

    let items = block_items(&pass.store, functions);
    let mut new_items = Vec::with_capacity(items.len());
    for f in items {
        let FirMatch::DeclareFun {
            name: fname,
            typ,
            args,
            body: Some(body),
            is_inline,
        } = match_fir(&pass.store, f)
        else {
            new_items.push(f);
            continue;
        };
        if !TARGET_FUNCTIONS.contains(&fname.as_str()) {
            new_items.push(f);
            continue;
        }
        let mut env = Env::default();
        env.push();
        for arg in &args {
            if arg.name != DSP_ARG {
                env.declare(arg.name.clone(), Binding::Var(arg.typ.clone()));
            }
        }
        let new_body = pass.process(&fname, &fname, &env, body, true);
        if new_body == body {
            new_items.push(f);
        } else {
            new_items.push(FirBuilder::new(&mut pass.store).declare_fun(
                fname,
                typ,
                &args,
                Some(new_body),
                is_inline,
            ));
        }
    }
    while let Some(k) = pass.queue.pop_front() {
        let (name, root_name, body, is_run) = {
            let a = &pass.added[k];
            (a.name.clone(), a.root.clone(), a.body, a.is_run)
        };
        let mut env = Env::default();
        env.push();
        for p in &pass.added[k].params {
            let binding = match p.mode {
                Mode::Value | Mode::Pointer | Mode::Spill(_) => Binding::Var(p.fun_type()),
                Mode::Ref | Mode::PassRef => Binding::Ref(p.typ.clone()),
            };
            env.declare(p.name.clone(), binding);
        }
        let new_body = pass.process(&name, &root_name, &env, body, !is_run);
        pass.added[k].body = new_body;
    }
    if pass.added.is_empty() {
        return Ok((pass.store, root, pass.report));
    }
    for a in &pass.added {
        let mut types = vec![dsp_type()];
        let mut args = vec![NamedType {
            name: DSP_ARG.to_owned(),
            typ: dsp_type(),
        }];
        for p in &a.params {
            types.push(p.fun_type());
            args.push(NamedType {
                name: p.name.clone(),
                typ: p.fun_type(),
            });
        }
        let typ = FirType::Fun {
            args: types,
            ret: Box::new(FirType::Void),
        };
        new_items.push(FirBuilder::new(&mut pass.store).declare_fun(
            a.name.clone(),
            typ,
            &args,
            Some(a.body),
            false,
        ));
        pass.report.outlined.push(OutlinedFunction {
            name: a.name.clone(),
            parent: a.parent.clone(),
            statements: a.statements,
            by_value: a
                .params
                .iter()
                .filter(|p| matches!(p.mode, Mode::Value | Mode::Pointer))
                .map(|p| p.name.clone())
                .collect(),
            by_reference: a
                .params
                .iter()
                .filter(|p| !matches!(p.mode, Mode::Value | Mode::Pointer))
                .map(|p| p.name.clone())
                .collect(),
        });
    }
    // Sub-modules (the generated-table classes of the C and C++ backends)
    // are kept as they are: the pass rewrites the root's functions only.
    let sub_module_items = block_items(&pass.store, sub_modules);
    let mut b = FirBuilder::new(&mut pass.store);
    let functions = b.block(&new_items);
    let root = b.module(
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        &sub_module_items,
    );
    Ok((pass.store, root, pass.report))
}
