//! The independent checker of the outlining pass
//! (`porting/mojo-large-program-outlining-plan-2026-11-09-en.md` §4).
//!
//! [`check_outlining`] shares no code with the producer
//! ([`super::outline_large_blocks`]) beyond the FIR matcher and builder. It
//! restates the conventions on its own: an added function is a method
//! (`dsp: Ptr(Obj)` first) returning `Void`, called exactly once as a block
//! statement `Drop(FunCall(..))`, whose other parameters are scalars passed
//! by value or `Ptr` of a scalar passed by reference, or a pointer of the
//! caller passed by value (`Ptr` of a scalar again, its argument a `LoadVar`
//! instead of an address); the argument of a parameter names the caller's
//! variable of the same name.
//!
//! The rules, in the order they are checked:
//!
//! 1. **sections** — module header, structure, globals, static declarations
//!    and sub-modules identical by id; the source functions first, in order,
//!    same name, type, arguments and inline flag;
//! 2. **signature** — every added function has a fresh name and the method
//!    signature above;
//! 3. **calls** — every added function is called exactly once, from a block
//!    of a source function or of another added function, with the argument
//!    forms above, and reached from a source function;
//! 4. **closed** — an added function's body redeclares no parameter, has no
//!    other free local variable, never stores into a value parameter, and
//!    reaches a reference parameter only at index 0 (or passes it on), a
//!    pointer parameter only by a load or a table access at any index;
//! 5. **spills** — a spill array (phase 2, plan §3.4) is declared once in
//!    its block, before the calls that receive it; each slot is stored once,
//!    by a trailing statement of one run, and read only after that run;
//! 6. **inline-back** — replacing every call by its function's body (callees
//!    first), parameter accesses turned back into accesses of the caller's
//!    variable, spill declarations and stores dropped and slot reads turned
//!    back into the spilled local, gives every source function body exactly
//!    (by id); every spilled local is single-assignment in the block.
//!
//! # Public API mapping status
//! - `check_outlining`, `OutlineViolation`: no C++ counterpart (Rust-only,
//!   `adapted`).

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::{AccessType, FirBuilder, FirId, FirMatch, FirStore, FirType, NamedType, match_fir};

/// The receiver argument, restated.
const DSP: &str = "dsp";

/// A rule [`check_outlining`] found broken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutlineViolation {
    /// Rule 1: something other than function bodies and added functions
    /// changed.
    Sections(String),
    /// Rule 2: an added function's name or signature.
    Signature { function: String, why: String },
    /// Rule 3: how an added function is called.
    Calls { function: String, why: String },
    /// Rule 4: an added function's body reaches something it should not.
    Closed { function: String, why: String },
    /// Rule 5, and the single-assignment check of rule 6: a spill array.
    Spills { function: String, why: String },
    /// Rule 5: inlining the calls back does not give the source body.
    InlineBack { function: String },
}

impl fmt::Display for OutlineViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sections(why) => write!(f, "sections: {why}"),
            Self::Signature { function, why } => write!(f, "signature of `{function}`: {why}"),
            Self::Calls { function, why } => write!(f, "calls of `{function}`: {why}"),
            Self::Closed { function, why } => write!(f, "closed `{function}`: {why}"),
            Self::Spills { function, why } => write!(f, "spills in `{function}`: {why}"),
            Self::InlineBack { function } => write!(
                f,
                "inline-back: `{function}` with its calls inlined differs from its source body"
            ),
        }
    }
}

impl std::error::Error for OutlineViolation {}

/// Checks that `dst_module` (in `dst`) is an exact outlining of
/// `src_module` (in `src`) (module doc).
///
/// # Errors
/// The first [`OutlineViolation`] found, in rule order.
pub fn check_outlining(
    src: &FirStore,
    src_module: FirId,
    dst: &FirStore,
    dst_module: FirId,
) -> Result<(), OutlineViolation> {
    tlib::on_compile_stack(|| Checker::run(src, src_module, dst, dst_module))
}

/// How a parameter of an added function is passed.
#[derive(Debug, Clone, PartialEq)]
enum Passing {
    Value(FirType),
    Reference(FirType),
    /// A pointer of the caller, by value: `Ptr(T)` whose argument is
    /// `LoadVar` of a pointer variable rather than an address. Rule 2 cannot
    /// tell it from a reference (the same type); rule 3 tells them apart by
    /// the argument.
    Pointer(FirType),
    /// A spill array of `n` elements, by reference.
    Spill(FirType, usize),
}

#[derive(Debug, Clone)]
struct Function {
    name: String,
    typ: FirType,
    args: Vec<NamedType>,
    body: Option<FirId>,
    is_inline: bool,
}

/// An added function, as rule 2 accepted it.
#[derive(Debug, Clone)]
struct Added {
    body: FirId,
    params: Vec<(String, Passing)>,
}

/// One call, as rule 3 accepted it: the caller and the arguments after
/// `dsp`.
#[derive(Debug, Clone)]
struct Call {
    caller: String,
    args: Vec<FirId>,
}

struct Checker {
    cs: FirStore,
    /// Every name the source module declares, its functions included.
    src_names: HashSet<String>,
    added: HashMap<String, Added>,
    /// The calls of each added function (rule 3 leaves exactly one).
    calls: HashMap<String, Vec<Call>>,
    inlined: HashMap<FirId, FirId>,
}

fn items(store: &FirStore, id: FirId) -> Vec<FirId> {
    match match_fir(store, id) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    }
}

fn scalar(typ: &FirType) -> bool {
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

fn local(access: AccessType) -> bool {
    matches!(
        access,
        AccessType::Stack | AccessType::Loop | AccessType::FunArgs
    )
}

fn obj_ptr() -> FirType {
    FirType::Ptr(Box::new(FirType::Obj))
}

fn every_declared_name(
    store: &FirStore,
    id: FirId,
    seen: &mut HashSet<FirId>,
    out: &mut HashSet<String>,
) {
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
    for &child in store.arena.children(id).unwrap_or(&[]) {
        every_declared_name(store, child, seen, out);
    }
}

/// `(header, functions)` of a module: the header holds every section but
/// the functions.
#[allow(clippy::type_complexity)]
fn module_parts(
    store: &FirStore,
    root: FirId,
    which: &str,
) -> Result<((usize, usize, String, FirId, FirId, FirId, FirId), FirId), OutlineViolation> {
    match match_fir(store, root) {
        FirMatch::Module {
            num_inputs,
            num_outputs,
            name,
            dsp_struct,
            globals,
            functions,
            static_decls,
            sub_modules,
        } => Ok((
            (
                num_inputs,
                num_outputs,
                name,
                dsp_struct,
                globals,
                static_decls,
                sub_modules,
            ),
            functions,
        )),
        _ => Err(OutlineViolation::Sections(format!(
            "the {which} root is not a module"
        ))),
    }
}

fn decode_function(store: &FirStore, id: FirId) -> Option<Function> {
    match match_fir(store, id) {
        FirMatch::DeclareFun {
            name,
            typ,
            args,
            body,
            is_inline,
        } => Some(Function {
            name,
            typ,
            args,
            body,
            is_inline,
        }),
        _ => None,
    }
}

/// A call statement `Drop(FunCall(name, args, typ))`.
fn call_statement(store: &FirStore, stmt: FirId) -> Option<(String, Vec<FirId>, FirType)> {
    let FirMatch::Drop(value) = match_fir(store, stmt) else {
        return None;
    };
    match match_fir(store, value) {
        FirMatch::FunCall { name, args, typ } => Some((name, args, typ)),
        _ => None,
    }
}

impl Checker {
    fn run(
        src: &FirStore,
        src_module: FirId,
        dst: &FirStore,
        dst_module: FirId,
    ) -> Result<(), OutlineViolation> {
        let mut cs = FirStore::new();
        let s = cs.import_from(src, src_module);
        let d = cs.import_from(dst, dst_module);
        let (s_header, s_functions) = module_parts(&cs, s, "source")?;
        let (d_header, d_functions) = module_parts(&cs, d, "output")?;
        if s_header != d_header {
            return Err(OutlineViolation::Sections(
                "the module header or a section other than the functions differs".to_owned(),
            ));
        }
        let s_funs: Vec<Function> = items(&cs, s_functions)
            .into_iter()
            .filter_map(|f| decode_function(&cs, f))
            .collect();
        let d_items = items(&cs, d_functions);
        let d_funs: Vec<Function> = d_items
            .iter()
            .filter_map(|&f| decode_function(&cs, f))
            .collect();
        if d_funs.len() != d_items.len() || s_funs.len() != items(&cs, s_functions).len() {
            return Err(OutlineViolation::Sections(
                "a functions section holds something other than functions".to_owned(),
            ));
        }
        if d_funs.len() < s_funs.len() {
            return Err(OutlineViolation::Sections(
                "the output has fewer functions than the source".to_owned(),
            ));
        }
        for (sf, df) in s_funs.iter().zip(&d_funs) {
            if sf.name != df.name
                || sf.typ != df.typ
                || sf.args != df.args
                || sf.is_inline != df.is_inline
                || sf.body.is_some() != df.body.is_some()
            {
                return Err(OutlineViolation::Sections(format!(
                    "source function `{}` is not the output function at its position",
                    sf.name
                )));
            }
        }

        let mut checker = Checker {
            cs,
            src_names: HashSet::new(),
            added: HashMap::new(),
            calls: HashMap::new(),
            inlined: HashMap::new(),
        };
        checker.signatures(s, &s_funs, &d_funs[s_funs.len()..])?;
        checker.calls_rule(&d_funs)?;
        for f in &d_funs[s_funs.len()..] {
            checker.closed(&f.name)?;
        }
        for f in &d_funs {
            if let Some(body) = f.body {
                checker.spills(&f.name, body)?;
            }
        }
        for (sf, df) in s_funs.iter().zip(&d_funs) {
            let (Some(sb), Some(db)) = (sf.body, df.body) else {
                continue;
            };
            if checker.inline(db).map_err(|why| OutlineViolation::Spills {
                function: sf.name.clone(),
                why,
            })? != sb
            {
                return Err(OutlineViolation::InlineBack {
                    function: sf.name.clone(),
                });
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------- rule 2

    fn signatures(
        &mut self,
        src_root: FirId,
        s_funs: &[Function],
        added: &[Function],
    ) -> Result<(), OutlineViolation> {
        let mut taken = HashSet::new();
        every_declared_name(&self.cs, src_root, &mut HashSet::new(), &mut taken);
        for f in s_funs {
            taken.insert(f.name.clone());
        }
        for f in added {
            let bad = |why: &str| OutlineViolation::Signature {
                function: f.name.clone(),
                why: why.to_owned(),
            };
            if taken.contains(&f.name) || self.added.contains_key(&f.name) {
                return Err(bad("the name is already used"));
            }
            let Some(body) = f.body else {
                return Err(bad("no body"));
            };
            let FirType::Fun { args, ret } = &f.typ else {
                return Err(bad("not a function type"));
            };
            if **ret != FirType::Void {
                return Err(bad("does not return Void"));
            }
            if args.len() != f.args.len() || args.is_empty() {
                return Err(bad("the argument list does not match the type"));
            }
            if f.args[0].name != DSP || f.args[0].typ != obj_ptr() || args[0] != obj_ptr() {
                return Err(bad("the first argument is not `dsp: Ptr(Obj)`"));
            }
            let mut names = HashSet::new();
            let mut params = Vec::new();
            for (named, typ) in f.args.iter().zip(args) {
                if named.typ != *typ {
                    return Err(bad("an argument type differs from the function type"));
                }
                if !names.insert(named.name.clone()) {
                    return Err(bad("two arguments share a name"));
                }
            }
            for named in &f.args[1..] {
                let passing = match &named.typ {
                    t if scalar(t) => Passing::Value(t.clone()),
                    FirType::Ptr(inner) if scalar(inner) => Passing::Reference((**inner).clone()),
                    FirType::Array(inner, n) if scalar(inner) => {
                        Passing::Spill((**inner).clone(), *n)
                    }
                    _ => {
                        return Err(bad(
                            "a parameter is neither a scalar, a pointer to one nor an array of one",
                        ));
                    }
                };
                params.push((named.name.clone(), passing));
            }
            self.added.insert(f.name.clone(), Added { body, params });
        }
        self.src_names = taken;
        Ok(())
    }

    // ------------------------------------------------------------- rule 3

    fn calls_rule(&mut self, d_funs: &[Function]) -> Result<(), OutlineViolation> {
        for f in d_funs {
            if let Some(body) = f.body {
                let mut seen = HashSet::new();
                self.scan_block(&f.name, body, &mut seen)?;
            }
        }
        let names: Vec<String> = self.added.keys().cloned().collect();
        for name in &names {
            let calls = self.calls.get(name).map_or(0, Vec::len);
            if calls != 1 {
                return Err(OutlineViolation::Calls {
                    function: name.clone(),
                    why: format!("called {calls} times, not once"),
                });
            }
        }
        // Reached from a source function: no cycle among added functions.
        for name in &names {
            let mut at = name.clone();
            let mut steps = 0;
            while self.added.contains_key(&at) {
                at = self.calls[&at][0].caller.clone();
                steps += 1;
                if steps > names.len() {
                    return Err(OutlineViolation::Calls {
                        function: name.clone(),
                        why: "not reached from a source function".to_owned(),
                    });
                }
            }
        }
        self.pointer_parameters(&names);
        for name in &names {
            let call = self.calls[name][0].clone();
            self.check_arguments(name, &call)?;
        }
        Ok(())
    }

    /// The `Ptr(T)` parameters whose argument is `LoadVar` of a pointer of
    /// the caller that is not one of its references become
    /// [`Passing::Pointer`]; callers first, since a pointer parameter passed
    /// on is told apart by its caller's passing.
    fn pointer_parameters(&mut self, names: &[String]) {
        let depth = |name: &str| {
            let mut at = name.to_owned();
            let mut d = 0;
            while self.added.contains_key(&at) {
                at = self.calls[&at][0].caller.clone();
                d += 1;
            }
            d
        };
        let mut order: Vec<(usize, String)> = names.iter().map(|n| (depth(n), n.clone())).collect();
        order.sort();
        for (_, name) in order {
            let call = self.calls[&name][0].clone();
            let caller_refs: HashSet<String> =
                self.added.get(&call.caller).map_or_else(HashSet::new, |a| {
                    a.params
                        .iter()
                        .filter(|(_, p)| matches!(p, Passing::Reference(_)))
                        .map(|(n, _)| n.clone())
                        .collect()
                });
            let mut params = self.added[&name].params.clone();
            for ((param, passing), &arg) in params.iter_mut().zip(&call.args) {
                if let Passing::Reference(t) = passing
                    && let FirMatch::LoadVar { name: var, typ, .. } = match_fir(&self.cs, arg)
                    && var == *param
                    && !caller_refs.contains(&var)
                    && typ == FirType::Ptr(Box::new(t.clone()))
                {
                    *passing = Passing::Pointer(t.clone());
                }
            }
            if let Some(a) = self.added.get_mut(&name) {
                a.params = params;
            }
        }
    }

    /// Records the call statements of a block (as items), and refuses any
    /// other occurrence of a call to an added function.
    fn scan_block(
        &mut self,
        caller: &str,
        block: FirId,
        seen: &mut HashSet<FirId>,
    ) -> Result<(), OutlineViolation> {
        for item in items(&self.cs, block) {
            if let Some((name, args, typ)) = call_statement(&self.cs, item)
                && self.added.contains_key(&name)
            {
                if typ != FirType::Void {
                    return Err(OutlineViolation::Calls {
                        function: name,
                        why: "the call is not of type Void".to_owned(),
                    });
                }
                let n = self.added[&name].params.len();
                if args.len() != n + 1 {
                    return Err(OutlineViolation::Calls {
                        function: name,
                        why: format!("{} arguments for {} parameters", args.len(), n + 1),
                    });
                }
                let dsp_ok = matches!(
                    match_fir(&self.cs, args[0]),
                    FirMatch::LoadVar { name, access: AccessType::FunArgs, typ }
                        if name == DSP && typ == obj_ptr()
                );
                if !dsp_ok {
                    return Err(OutlineViolation::Calls {
                        function: name,
                        why: "the first argument is not the caller's `dsp`".to_owned(),
                    });
                }
                self.calls.entry(name).or_default().push(Call {
                    caller: caller.to_owned(),
                    args: args[1..].to_vec(),
                });
                continue;
            }
            self.scan_stmt(caller, item, seen)?;
        }
        Ok(())
    }

    fn scan_stmt(
        &mut self,
        caller: &str,
        id: FirId,
        seen: &mut HashSet<FirId>,
    ) -> Result<(), OutlineViolation> {
        match match_fir(&self.cs, id) {
            FirMatch::Block(_) => return self.scan_block(caller, id, seen),
            FirMatch::FunCall { name, .. } if self.added.contains_key(&name) => {
                return Err(OutlineViolation::Calls {
                    function: name,
                    why: "called other than as a statement of a block".to_owned(),
                });
            }
            _ => {}
        }
        // Values hold no statement: each is scanned once. Everything else
        // (statements, case lists, blocks) is walked at every occurrence, so
        // that a call statement written twice is counted twice.
        if self.cs.value_type(id).is_some() && !seen.insert(id) {
            return Ok(());
        }
        for child in self.cs.arena.children(id).unwrap_or(&[]).to_vec() {
            self.scan_stmt(caller, child, seen)?;
        }
        Ok(())
    }

    fn check_arguments(&self, name: &str, call: &Call) -> Result<(), OutlineViolation> {
        let bad = |why: String| OutlineViolation::Calls {
            function: name.to_owned(),
            why,
        };
        let caller_refs: HashSet<&str> =
            self.added.get(&call.caller).map_or_else(HashSet::new, |a| {
                a.params
                    .iter()
                    .filter(|(_, p)| matches!(p, Passing::Reference(_)))
                    .map(|(n, _)| n.as_str())
                    .collect()
            });
        for ((param, passing), &arg) in self.added[name].params.iter().zip(&call.args) {
            let ok = match (passing, match_fir(&self.cs, arg)) {
                (Passing::Value(t), FirMatch::LoadVar { name, access, typ }) => {
                    name == *param && local(access) && typ == *t
                }
                (Passing::Pointer(t), FirMatch::LoadVar { name, access, typ }) => {
                    name == *param
                        && local(access)
                        && !caller_refs.contains(name.as_str())
                        && typ == FirType::Ptr(Box::new(t.clone()))
                }
                (Passing::Reference(t), FirMatch::LoadVarAddress { name, access, typ }) => {
                    name == *param && local(access) && typ == FirType::Ptr(Box::new(t.clone()))
                }
                (
                    Passing::Reference(t),
                    FirMatch::LoadVar {
                        name,
                        access: AccessType::FunArgs,
                        typ,
                    },
                ) => {
                    name == *param
                        && caller_refs.contains(name.as_str())
                        && typ == FirType::Ptr(Box::new(t.clone()))
                }
                (
                    Passing::Spill(t, n),
                    FirMatch::LoadVar {
                        name,
                        access: AccessType::Stack,
                        typ,
                    },
                ) => {
                    name == *param
                        && !self.src_names.contains(&name)
                        && typ == FirType::Array(Box::new(t.clone()), *n)
                }
                _ => false,
            };
            if !ok {
                return Err(bad(format!(
                    "the argument of parameter `{param}` ({passing:?}) does not name the \
                     caller's variable the way the parameter is passed: {:?}",
                    match_fir(&self.cs, arg)
                )));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------- rule 4

    fn closed(&self, name: &str) -> Result<(), OutlineViolation> {
        let added = &self.added[name];
        let mut walk = Closed {
            cs: &self.cs,
            added: &self.added,
            params: added.params.iter().cloned().collect(),
            scopes: Vec::new(),
        };
        walk.block(added.body)
            .map_err(|why| OutlineViolation::Closed {
                function: name.to_owned(),
                why,
            })
    }

    // ------------------------------------------------------------- rule 5

    /// The trailing spill stores of an added function's raw body:
    /// `(spill parameter, slot, local)`, in order.
    fn trailing_spills(&self, name: &str) -> Vec<(String, i32, String)> {
        let added = &self.added[name];
        let spills: HashSet<&str> = added
            .params
            .iter()
            .filter(|(_, p)| matches!(p, Passing::Spill(..)))
            .map(|(n, _)| n.as_str())
            .collect();
        let mut out = Vec::new();
        for item in items(&self.cs, added.body).into_iter().rev() {
            let FirMatch::StoreTable {
                name: spill,
                access: AccessType::FunArgs,
                index,
                value,
            } = match_fir(&self.cs, item)
            else {
                break;
            };
            let (
                FirMatch::Int32 { value: k, .. },
                FirMatch::LoadVar {
                    name: e,
                    access: AccessType::Stack,
                    ..
                },
            ) = (match_fir(&self.cs, index), match_fir(&self.cs, value))
            else {
                break;
            };
            if !spills.contains(spill.as_str()) {
                break;
            }
            out.push((spill, k, e));
        }
        out.reverse();
        out
    }

    /// The spill declarations of a block: `DeclareVar(p, Array, Stack,
    /// None)` of a name the source does not declare, by item index.
    fn spill_declarations(&self, block: &[FirId]) -> HashMap<String, usize> {
        let mut out = HashMap::new();
        for (i, &item) in block.iter().enumerate() {
            if let FirMatch::DeclareVar {
                name,
                typ: FirType::Array(..),
                access: AccessType::Stack,
                init: None,
            } = match_fir(&self.cs, item)
                && !self.src_names.contains(&name)
            {
                out.insert(name, i);
            }
        }
        out
    }

    // ------------------------------------------------------------- rule 5

    /// Rule 5 over every block of a function.
    fn spills(&mut self, fname: &str, body: FirId) -> Result<(), OutlineViolation> {
        let bad = |why: String| OutlineViolation::Spills {
            function: fname.to_owned(),
            why,
        };
        let block = items(&self.cs, body);
        let decls = self.spill_declarations(&block);
        if !decls.is_empty() {
            self.spills_in_block(&block, &decls).map_err(bad)?;
        }
        for item in block {
            for child in nested_blocks(&self.cs, item) {
                self.spills(fname, child)?;
            }
        }
        Ok(())
    }

    fn spills_in_block(
        &self,
        block: &[FirId],
        decls: &HashMap<String, usize>,
    ) -> Result<(), String> {
        // Slots stored: (spill, slot) -> (local, call index).
        let mut stored: HashMap<(String, i32), (String, usize)> = HashMap::new();
        for (c, &item) in block.iter().enumerate() {
            let Some((g, args, _)) = call_statement(&self.cs, item) else {
                continue;
            };
            let Some(added) = self.added.get(&g) else {
                continue;
            };
            for ((param, passing), &arg) in added.params.iter().zip(&args[1..]) {
                if matches!(passing, Passing::Spill(..)) {
                    match decls.get(param) {
                        Some(&d) if d < c => {}
                        _ => {
                            return Err(format!(
                                "spill `{param}` passed to `{g}` without a declaration before"
                            ));
                        }
                    }
                    let _ = arg;
                }
            }
            for (spill, k, e) in self.trailing_spills(&g) {
                if stored.insert((spill.clone(), k), (e, c)).is_some() {
                    return Err(format!("slot {k} of `{spill}` stored twice"));
                }
            }
        }
        for (c, &item) in block.iter().enumerate() {
            let mut accesses = Vec::new();
            match call_statement(&self.cs, item) {
                Some((g, _, _)) if self.added.contains_key(&g) => {
                    let trailing = self.trailing_spills(&g).len();
                    let body = items(&self.cs, self.added[&g].body);
                    for &stmt in &body[..body.len() - trailing] {
                        spill_accesses(&self.cs, stmt, decls, &mut accesses);
                    }
                }
                _ => {
                    if decls.values().any(|&d| d == c) {
                        continue;
                    }
                    spill_accesses(&self.cs, item, decls, &mut accesses);
                }
            }
            for (spill, k, is_read) in accesses {
                let Some(k) = k.filter(|_| is_read) else {
                    return Err(format!("`{spill}` accessed other than by a slot read"));
                };
                match stored.get(&(spill.clone(), k)) {
                    Some(&(_, d)) if d < c => {}
                    Some(_) => {
                        return Err(format!(
                            "slot {k} of `{spill}` read before or in the run that stores it"
                        ));
                    }
                    None => return Err(format!("slot {k} of `{spill}` read but never stored")),
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------- rule 6

    /// `id` with every call to an added function replaced by the function's
    /// inlined body, parameters substituted, spills undone; `Err` when a
    /// spilled local is not single-assignment in its block.
    fn inline(&mut self, id: FirId) -> Result<FirId, String> {
        if let Some(&done) = self.inlined.get(&id) {
            return Ok(done);
        }
        let out = if let FirMatch::Block(block) = match_fir(&self.cs, id) {
            let decls = self.spill_declarations(&block);
            let mut slots: HashMap<(String, i32), String> = HashMap::new();
            let mut out = Vec::with_capacity(block.len());
            let mut changed = false;
            for (i, &item) in block.iter().enumerate() {
                if decls.values().any(|&d| d == i) {
                    changed = true;
                    continue;
                }
                match call_statement(&self.cs, item) {
                    Some((name, args, _)) if self.added.contains_key(&name) => {
                        changed = true;
                        let trailing = self.trailing_spills(&name);
                        let body = self.added[&name].body;
                        let inlined = self.inline(body)?;
                        let params = self.added[&name].params.clone();
                        let restored = self.substitute(inlined, &params, &args[1..]);
                        let restored = items(&self.cs, restored);
                        out.extend_from_slice(&restored[..restored.len() - trailing.len()]);
                        for (spill, k, e) in trailing {
                            slots.insert((spill, k), e);
                        }
                    }
                    _ => {
                        let new = self.inline(item)?;
                        changed |= new != item;
                        out.push(new);
                    }
                }
            }
            if !slots.is_empty() {
                let mut back = SlotsBack {
                    cs: &mut self.cs,
                    slots: &slots,
                    memo: HashMap::new(),
                };
                for item in &mut out {
                    *item = back.node(*item);
                }
                let spilled: HashSet<&String> = slots.values().collect();
                single_assignment(&self.cs, &out, &spilled)?;
            }
            if changed {
                FirBuilder::new(&mut self.cs).block(&out)
            } else {
                id
            }
        } else {
            match self.cs.arena.children(id).map(<[FirId]>::to_vec) {
                Some(children) if !children.is_empty() => {
                    let mut changed = false;
                    let mut new_children = Vec::with_capacity(children.len());
                    for c in children {
                        let n = self.inline(c)?;
                        changed |= n != c;
                        new_children.push(n);
                    }
                    match self.cs.arena.kind(id).cloned() {
                        Some(kind) if changed => self.cs.arena.intern(kind, &new_children),
                        _ => id,
                    }
                }
                _ => id,
            }
        };
        self.inlined.insert(id, out);
        Ok(out)
    }

    /// The body with each parameter access turned back into an access of
    /// the argument's variable.
    fn substitute(&mut self, body: FirId, params: &[(String, Passing)], args: &[FirId]) -> FirId {
        let mut map: HashMap<String, Restore> = HashMap::new();
        for ((name, passing), &arg) in params.iter().zip(args) {
            let restore = match (passing, match_fir(&self.cs, arg)) {
                (Passing::Value(_), FirMatch::LoadVar { access, .. }) => Restore::Value(access),
                (Passing::Pointer(_), FirMatch::LoadVar { access, .. }) => Restore::Pointer(access),
                (Passing::Reference(_), FirMatch::LoadVarAddress { access, .. }) => {
                    Restore::Reference(access)
                }
                _ => Restore::Same,
            };
            map.insert(name.clone(), restore);
        }
        let mut sub = Substitute {
            cs: &mut self.cs,
            map: &map,
            memo: HashMap::new(),
        };
        sub.node(body)
    }
}

/// What a parameter access becomes in the caller.
#[derive(Debug, Clone, Copy)]
enum Restore {
    /// `LoadVar(p, FunArgs)` becomes `LoadVar(p, access)`.
    Value(AccessType),
    /// Index-0 table accesses become `LoadVar`/`StoreVar(p, access)`.
    Reference(AccessType),
    /// `LoadVar` and the table accesses of `p` at `FunArgs` become accesses
    /// at `access`, the index kept.
    Pointer(AccessType),
    /// A reference passed on: unchanged.
    Same,
}

struct Substitute<'a> {
    cs: &'a mut FirStore,
    map: &'a HashMap<String, Restore>,
    memo: HashMap<FirId, FirId>,
}

impl Substitute<'_> {
    fn node(&mut self, id: FirId) -> FirId {
        if let Some(&done) = self.memo.get(&id) {
            return done;
        }
        let out = match match_fir(self.cs, id) {
            FirMatch::LoadVar {
                name,
                access: AccessType::FunArgs,
                typ,
            } => match self.map.get(&name) {
                Some(Restore::Value(access) | Restore::Pointer(access)) => {
                    FirBuilder::new(self.cs).load_var(name, *access, typ)
                }
                _ => id,
            },
            FirMatch::LoadTable {
                name,
                access: AccessType::FunArgs,
                typ,
                index,
            } => match self.map.get(&name) {
                Some(Restore::Reference(access)) => {
                    FirBuilder::new(self.cs).load_var(name, *access, typ)
                }
                Some(Restore::Pointer(access)) => {
                    let access = *access;
                    let index = self.node(index);
                    FirBuilder::new(self.cs).load_table(name, access, index, typ)
                }
                _ => self.children(id),
            },
            FirMatch::StoreTable {
                name,
                access: AccessType::FunArgs,
                value,
                index,
            } => match self.map.get(&name) {
                Some(Restore::Reference(access)) => {
                    let access = *access;
                    let value = self.node(value);
                    FirBuilder::new(self.cs).store_var(name, access, value)
                }
                Some(Restore::Pointer(access)) => {
                    let access = *access;
                    let index = self.node(index);
                    let value = self.node(value);
                    FirBuilder::new(self.cs).store_table(name, access, index, value)
                }
                _ => self.children(id),
            },
            _ => self.children(id),
        };
        self.memo.insert(id, out);
        out
    }

    fn children(&mut self, id: FirId) -> FirId {
        let Some(children) = self.cs.arena.children(id).map(<[FirId]>::to_vec) else {
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
        match self.cs.arena.kind(id).cloned() {
            Some(kind) if changed => self.cs.arena.intern(kind, &out),
            _ => id,
        }
    }
}

/// Rule 4 over one added function's body.
struct Closed<'a> {
    cs: &'a FirStore,
    added: &'a HashMap<String, Added>,
    params: HashMap<String, Passing>,
    scopes: Vec<HashSet<String>>,
}

impl Closed<'_> {
    fn bound(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains(name))
    }

    fn bind(&mut self, name: String) -> Result<(), String> {
        if name == DSP || self.params.contains_key(&name) {
            return Err(format!("the body declares `{name}`, a parameter name"));
        }
        if let Some(s) = self.scopes.last_mut() {
            s.insert(name);
        }
        Ok(())
    }

    fn block(&mut self, id: FirId) -> Result<(), String> {
        self.scopes.push(HashSet::new());
        for item in items(self.cs, id) {
            if let Some((name, args, _)) = call_statement(self.cs, item)
                && self.added.contains_key(&name)
            {
                // Arguments name variables, checked by rule 3; here only
                // whether the body may name them.
                for arg in args {
                    self.argument(arg)?;
                }
                continue;
            }
            self.stmt(item)?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn argument(&mut self, arg: FirId) -> Result<(), String> {
        match match_fir(self.cs, arg) {
            FirMatch::LoadVar {
                name,
                access: AccessType::FunArgs,
                ..
            } if !self.bound(&name)
                && matches!(self.params.get(&name), Some(Passing::Reference(_))) =>
            {
                Ok(())
            }
            FirMatch::LoadVarAddress { name, access, .. }
                if local(access) && !self.bound(&name) =>
            {
                Err(format!("the address of `{name}`, not a local of the body"))
            }
            _ => self.value(arg),
        }
    }

    fn stmt(&mut self, id: FirId) -> Result<(), String> {
        match match_fir(self.cs, id) {
            FirMatch::Block(_) => self.block(id),
            FirMatch::DeclareVar { name, init, .. } => {
                if let Some(init) = init {
                    self.value(init)?;
                }
                self.bind(name)
            }
            FirMatch::DeclareTable { name, values, .. } => {
                for v in values {
                    self.value(v)?;
                }
                self.bind(name)
            }
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                self.value(cond)?;
                self.block(then_block)?;
                match else_block {
                    Some(b) => self.block(b),
                    None => Ok(()),
                }
            }
            FirMatch::Control { cond, stmt } => {
                self.value(cond)?;
                self.scopes.push(HashSet::new());
                let r = self.stmt(stmt);
                self.scopes.pop();
                r
            }
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => {
                self.value(cond)?;
                for (_, b) in cases {
                    self.block(b)?;
                }
                match default {
                    Some(b) => self.block(b),
                    None => Ok(()),
                }
            }
            FirMatch::ForLoop {
                init,
                end,
                step,
                body,
                ..
            } => {
                self.scopes.push(HashSet::new());
                let r = (|| {
                    self.stmt(init)?;
                    self.value(end)?;
                    self.value(step)?;
                    self.block(body)
                })();
                self.scopes.pop();
                r
            }
            FirMatch::SimpleForLoop {
                var, upper, body, ..
            } => {
                self.value(upper)?;
                self.scopes.push(HashSet::new());
                let r = self.bind(var).and_then(|()| self.block(body));
                self.scopes.pop();
                r
            }
            FirMatch::IteratorForLoop {
                iterators, body, ..
            } => {
                self.scopes.push(HashSet::new());
                let r = iterators
                    .into_iter()
                    .try_for_each(|it| self.bind(it))
                    .and_then(|()| self.block(body));
                self.scopes.pop();
                r
            }
            FirMatch::WhileLoop { cond, body } => {
                self.value(cond)?;
                self.block(body)
            }
            _ => self.value(id),
        }
    }

    /// A value, or a statement without nested block.
    fn value(&mut self, id: FirId) -> Result<(), String> {
        match match_fir(self.cs, id) {
            FirMatch::LoadVar { name, access, .. } => self.reach(&name, access, "a load")?,
            FirMatch::StoreVar { name, access, .. } => self.reach(&name, access, "a store")?,
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
                if local(access)
                    && !self.bound(&name)
                    && let Some(Passing::Spill(_, n)) = self.params.get(&name)
                {
                    let in_range = matches!(
                        match_fir(self.cs, index),
                        FirMatch::Int32 { value, .. }
                            if usize::try_from(value).is_ok_and(|k| k < *n)
                    );
                    if !in_range || access != AccessType::FunArgs {
                        return Err(format!(
                            "spill `{name}` reached other than at a literal index in range"
                        ));
                    }
                } else if local(access)
                    && !self.bound(&name)
                    && matches!(self.params.get(&name), Some(Passing::Pointer(_)))
                {
                    if access != AccessType::FunArgs {
                        return Err(format!(
                            "pointer `{name}` reached other than as a parameter"
                        ));
                    }
                } else if local(access)
                    && !self.bound(&name)
                    && matches!(self.params.get(&name), Some(Passing::Reference(_)))
                {
                    let zero =
                        matches!(match_fir(self.cs, index), FirMatch::Int32 { value: 0, .. });
                    if !zero || access != AccessType::FunArgs {
                        return Err(format!("reference `{name}` reached other than at index 0"));
                    }
                } else {
                    self.reach(&name, access, "an indexed access")?;
                }
            }
            FirMatch::LoadVarAddress { name, access, .. } => {
                self.reach(&name, access, "an address")?;
            }
            FirMatch::TeeVar { name, access, .. } => self.reach(&name, access, "a tee")?,
            FirMatch::ShiftArrayVar { name, access, .. } => {
                self.reach(&name, access, "a shift")?;
            }
            FirMatch::FunCall { name, .. } if self.added.contains_key(&name) => {
                return Err(format!("calls `{name}` other than as a block statement"));
            }
            _ => {}
        }
        for &child in self.cs.arena.children(id).unwrap_or(&[]) {
            self.value(child)?;
        }
        Ok(())
    }

    /// One access to a variable by something other than an index-0 table
    /// access of a reference.
    fn reach(&self, name: &str, access: AccessType, what: &str) -> Result<(), String> {
        if !local(access) || self.bound(name) {
            return Ok(());
        }
        if name == DSP {
            return if access == AccessType::FunArgs && what == "a load" {
                Ok(())
            } else {
                Err(format!("{what} of `dsp`"))
            };
        }
        match self.params.get(name) {
            Some(Passing::Value(_) | Passing::Pointer(_))
                if access == AccessType::FunArgs && what == "a load" =>
            {
                Ok(())
            }
            Some(Passing::Value(_)) => Err(format!("{what} of value parameter `{name}`")),
            Some(Passing::Pointer(_)) => Err(format!("{what} of pointer parameter `{name}`")),
            Some(Passing::Reference(_)) => Err(format!("{what} of reference `{name}`")),
            Some(Passing::Spill(..)) => Err(format!("{what} of spill `{name}`")),
            None => Err(format!("{what} of `{name}`, free in the body")),
        }
    }
}

/// The blocks nested directly in a statement.
fn nested_blocks(store: &FirStore, stmt: FirId) -> Vec<FirId> {
    match match_fir(store, stmt) {
        FirMatch::Block(_) => vec![stmt],
        FirMatch::If {
            then_block,
            else_block,
            ..
        } => std::iter::once(then_block).chain(else_block).collect(),
        FirMatch::Control { stmt, .. } => nested_blocks(store, stmt),
        FirMatch::ForLoop { body, .. }
        | FirMatch::SimpleForLoop { body, .. }
        | FirMatch::IteratorForLoop { body, .. }
        | FirMatch::WhileLoop { body, .. } => vec![body],
        FirMatch::Switch { cases, default, .. } => {
            cases.into_iter().map(|(_, b)| b).chain(default).collect()
        }
        _ => Vec::new(),
    }
}

/// Every access under `id` to a spill array of `decls`: `(spill, literal
/// slot, whether a `LoadTable`)`; the slot is `None` for any other access or
/// index.
fn spill_accesses(
    store: &FirStore,
    id: FirId,
    decls: &HashMap<String, usize>,
    out: &mut Vec<(String, Option<i32>, bool)>,
) {
    let found = match match_fir(store, id) {
        FirMatch::LoadTable { name, index, .. } if decls.contains_key(&name) => {
            let k = match match_fir(store, index) {
                FirMatch::Int32 { value, .. } => Some(value),
                _ => None,
            };
            Some((name, k, true))
        }
        FirMatch::LoadVar { name, .. }
        | FirMatch::StoreVar { name, .. }
        | FirMatch::StoreTable { name, .. }
        | FirMatch::LoadVarAddress { name, .. }
        | FirMatch::TeeVar { name, .. }
        | FirMatch::ShiftArrayVar { name, .. }
            if decls.contains_key(&name) =>
        {
            Some((name, None, false))
        }
        _ => None,
    };
    if let Some(f) = found {
        out.push(f);
    }
    for &child in store.arena.children(id).unwrap_or(&[]) {
        spill_accesses(store, child, decls, out);
    }
}

/// Whether every local of `names` is single-assignment in a block's items:
/// declared exactly once, with an initialiser, of a scalar type; never stored
/// into, tee'd, addressed or shifted. One walk for all of them.
fn single_assignment(
    store: &FirStore,
    block: &[FirId],
    names: &HashSet<&String>,
) -> Result<(), String> {
    #[derive(Default)]
    struct Seen {
        decls: Vec<(FirType, bool)>,
        mutated: bool,
    }
    fn walk(
        store: &FirStore,
        id: FirId,
        names: &HashSet<&String>,
        seen: &mut HashMap<String, Seen>,
    ) {
        match match_fir(store, id) {
            FirMatch::DeclareVar {
                name, typ, init, ..
            } if names.contains(&name) => {
                seen.entry(name)
                    .or_default()
                    .decls
                    .push((typ, init.is_some()));
            }
            FirMatch::DeclareTable {
                name, elem_type, ..
            } if names.contains(&name) => {
                let typ = FirType::Array(Box::new(elem_type), 0);
                seen.entry(name).or_default().decls.push((typ, true));
            }
            FirMatch::ForLoop { var, .. } | FirMatch::SimpleForLoop { var, .. }
                if names.contains(&var) =>
            {
                seen.entry(var)
                    .or_default()
                    .decls
                    .push((FirType::Int32, false));
            }
            FirMatch::StoreVar { name, access, .. }
            | FirMatch::StoreTable { name, access, .. }
            | FirMatch::TeeVar { name, access, .. }
            | FirMatch::LoadVarAddress { name, access, .. }
            | FirMatch::ShiftArrayVar { name, access, .. }
                if local(access) && names.contains(&name) =>
            {
                seen.entry(name).or_default().mutated = true;
            }
            _ => {}
        }
        for &child in store.arena.children(id).unwrap_or(&[]) {
            walk(store, child, names, seen);
        }
    }
    let mut seen: HashMap<String, Seen> = HashMap::new();
    for &item in block {
        walk(store, item, names, &mut seen);
    }
    let mut sorted: Vec<&&String> = names.iter().collect();
    sorted.sort();
    for e in sorted {
        let ok = seen.get(e.as_str()).is_some_and(|s| {
            !s.mutated && matches!(s.decls.as_slice(), [(typ, true)] if scalar(typ))
        });
        if !ok {
            return Err(format!(
                "spilled local `{e}` is not single-assignment in its block"
            ));
        }
    }
    Ok(())
}

/// Slot reads turned back into the spilled locals.
struct SlotsBack<'a> {
    cs: &'a mut FirStore,
    slots: &'a HashMap<(String, i32), String>,
    memo: HashMap<FirId, FirId>,
}

impl SlotsBack<'_> {
    fn node(&mut self, id: FirId) -> FirId {
        if let Some(&done) = self.memo.get(&id) {
            return done;
        }
        let out = match match_fir(self.cs, id) {
            FirMatch::LoadTable {
                name,
                access: AccessType::Stack | AccessType::FunArgs,
                index,
                typ,
            } => {
                let slot = match match_fir(self.cs, index) {
                    FirMatch::Int32 { value, .. } => self.slots.get(&(name, value)).cloned(),
                    _ => None,
                };
                match slot {
                    Some(e) => FirBuilder::new(self.cs).load_var(e, AccessType::Stack, typ),
                    None => self.children(id),
                }
            }
            _ => self.children(id),
        };
        self.memo.insert(id, out);
        out
    }

    fn children(&mut self, id: FirId) -> FirId {
        let Some(children) = self.cs.arena.children(id).map(<[FirId]>::to_vec) else {
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
        match self.cs.arena.kind(id).cloned() {
            Some(kind) if changed => self.cs.arena.intern(kind, &out),
            _ => id,
        }
    }
}
