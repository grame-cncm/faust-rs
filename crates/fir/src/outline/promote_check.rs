//! The independent checker of the reference promotion (plan
//! `porting/fir-outline-reference-promotion-plan-2026-11-10-en.md` §4).
//!
//! [`check_promotion`] shares no code with the producer
//! ([`super::promote`]) beyond the FIR matcher and builder. It reads the
//! promoted set off the output's struct, then rebuilds the source from the
//! output — each promoted field turned back into the local of the method
//! that stores it at its top level, and into the reference parameter of the
//! functions that no longer receive it — and requires the source module by
//! id. Rules, in order: sections, signatures, declarations, demotion.
//!
//! # Public API mapping status
//! - `check_promotion`, `PromotionViolation`: no C++ counterpart (Rust-only,
//!   `adapted`).

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::{AccessType, FirBuilder, FirId, FirMatch, FirStore, FirType, NamedType, match_fir};

/// The first rule an output breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionViolation {
    /// Rule 1: a section other than the struct differs, or the struct is not
    /// the source struct followed by new scalar fields.
    Sections(String),
    /// Rule 2: a function differs from its source other than by removed
    /// reference parameters of promoted fields.
    Signatures {
        /// The function.
        function: String,
        /// What differs.
        why: String,
    },
    /// Rule 3: a promoted name is still declared in a function.
    Declarations {
        /// The function.
        function: String,
        /// The name.
        name: String,
    },
    /// Rule 4: the demoted output is not the source.
    Demotion(String),
}

impl fmt::Display for PromotionViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sections(why) => write!(f, "sections: {why}"),
            Self::Signatures { function, why } => write!(f, "signatures: `{function}`: {why}"),
            Self::Declarations { function, name } => {
                write!(f, "declarations: `{function}` still declares `{name}`")
            }
            Self::Demotion(why) => write!(f, "demotion: {why}"),
        }
    }
}

impl std::error::Error for PromotionViolation {}

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

fn declared_names(
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
        _ => {}
    }
    for &c in store.arena.children(id).unwrap_or(&[]) {
        declared_names(store, c, seen, out);
    }
}

struct Parts {
    header: (usize, usize, String, FirId, FirId, FirId),
    dsp_struct: FirId,
    functions: FirId,
}

fn parts(store: &FirStore, root: FirId) -> Option<Parts> {
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
        } => Some(Parts {
            header: (
                num_inputs,
                num_outputs,
                name,
                globals,
                static_decls,
                sub_modules,
            ),
            dsp_struct,
            functions,
        }),
        _ => None,
    }
}

#[derive(Clone)]
struct Fun {
    name: String,
    typ: FirType,
    args: Vec<NamedType>,
    body: Option<FirId>,
    is_inline: bool,
}

fn funs(store: &FirStore, block: FirId) -> Option<Vec<Fun>> {
    items(store, block)
        .into_iter()
        .map(|f| match match_fir(store, f) {
            FirMatch::DeclareFun {
                name,
                typ,
                args,
                body,
                is_inline,
            } => Some(Fun {
                name,
                typ,
                args,
                body,
                is_inline,
            }),
            _ => None,
        })
        .collect()
}

/// Checks that `dst` is `src` with some method locals promoted to struct
/// fields (module doc).
///
/// # Errors
/// The first [`PromotionViolation`] found, in rule order.
pub fn check_promotion(
    src: &FirStore,
    src_module: FirId,
    dst: &FirStore,
    dst_module: FirId,
) -> Result<(), PromotionViolation> {
    tlib::on_compile_stack(|| run(src, src_module, dst, dst_module))
}

fn run(
    src: &FirStore,
    src_module: FirId,
    dst: &FirStore,
    dst_module: FirId,
) -> Result<(), PromotionViolation> {
    let mut cs = FirStore::new();
    let s = cs.import_from(src, src_module);
    let d = cs.import_from(dst, dst_module);
    let sections = |why: &str| PromotionViolation::Sections(why.to_owned());
    let sp = parts(&cs, s).ok_or_else(|| sections("the source root is not a module"))?;
    let dp = parts(&cs, d).ok_or_else(|| sections("the output root is not a module"))?;

    // Rule 1.
    if sp.header != dp.header {
        return Err(sections(
            "the header, globals, static declarations or sub-modules differ",
        ));
    }
    let s_struct = items(&cs, sp.dsp_struct);
    let d_struct = items(&cs, dp.dsp_struct);
    if d_struct.len() < s_struct.len() || d_struct[..s_struct.len()] != s_struct[..] {
        return Err(sections("the struct does not start with the source struct"));
    }
    // A promoted name may be a local of a function body (that is the point),
    // but no field, global, static or sub-module item, and no function.
    let mut src_names = HashSet::new();
    let (_, _, _, s_globals, s_statics, s_subs) = sp.header.clone();
    for section in [sp.dsp_struct, s_globals, s_statics, s_subs] {
        declared_names(&cs, section, &mut HashSet::new(), &mut src_names);
    }
    for f in items(&cs, sp.functions) {
        if let FirMatch::DeclareFun { name, .. } = match_fir(&cs, f) {
            src_names.insert(name);
        }
    }
    let mut promoted: HashMap<String, FirType> = HashMap::new();
    for &field in &d_struct[s_struct.len()..] {
        match match_fir(&cs, field) {
            FirMatch::DeclareVar {
                name,
                typ,
                access: AccessType::Struct,
                init: None,
            } if scalar(&typ) && !src_names.contains(&name) && !promoted.contains_key(&name) => {
                promoted.insert(name, typ);
            }
            _ => return Err(sections("an added struct item is not a new scalar field")),
        }
    }
    if promoted.is_empty() {
        return if s == d {
            Ok(())
        } else {
            Err(sections("nothing promoted, yet the module differs"))
        };
    }

    // Rule 2.
    let s_funs = funs(&cs, sp.functions).ok_or_else(|| sections("a source function item"))?;
    let d_funs = funs(&cs, dp.functions).ok_or_else(|| sections("an output function item"))?;
    if s_funs.len() != d_funs.len() {
        return Err(sections("the number of functions differs"));
    }
    let mut removed: HashMap<String, Vec<String>> = HashMap::new();
    let mut src_sig: HashMap<String, Vec<NamedType>> = HashMap::new();
    for (sf, df) in s_funs.iter().zip(&d_funs) {
        let bad = |why: &str| PromotionViolation::Signatures {
            function: df.name.clone(),
            why: why.to_owned(),
        };
        if sf.name != df.name || sf.is_inline != df.is_inline {
            return Err(bad("name or inline flag"));
        }
        if sf.body.is_some() != df.body.is_some() {
            return Err(bad("body presence"));
        }
        let (FirType::Fun { ret: sr, .. }, FirType::Fun { args: dt, ret: dr }) = (&sf.typ, &df.typ)
        else {
            if sf.typ != df.typ || sf.args != df.args {
                return Err(bad("type"));
            }
            continue;
        };
        if sr != dr {
            return Err(bad("return type"));
        }
        if dt.len() != df.args.len() || dt.iter().zip(&df.args).any(|(t, a)| *t != a.typ) {
            return Err(bad("the argument list does not match the type"));
        }
        let mut gone = Vec::new();
        let mut rest = df.args.iter().peekable();
        for a in &sf.args {
            if rest.peek().is_some_and(|r| *r == a) {
                rest.next();
            } else if promoted
                .get(&a.name)
                .is_some_and(|t| a.typ == FirType::Ptr(Box::new(t.clone())))
            {
                gone.push(a.name.clone());
            } else {
                return Err(bad("an argument is missing that is no promoted reference"));
            }
        }
        if rest.next().is_some() {
            return Err(bad("an argument the source does not have"));
        }
        if !gone.is_empty() {
            removed.insert(df.name.clone(), gone);
        }
        src_sig.insert(sf.name.clone(), sf.args.clone());
    }

    // Rule 3.
    for df in &d_funs {
        if let Some(body) = df.body {
            let mut names = HashSet::new();
            declared_names(&cs, body, &mut HashSet::new(), &mut names);
            if let Some(name) = names.into_iter().find(|n| promoted.contains_key(n)) {
                return Err(PromotionViolation::Declarations {
                    function: df.name.clone(),
                    name,
                });
            }
        }
    }

    // Rule 4.
    let mut rebuilt = Vec::with_capacity(d_funs.len());
    for (sf, df) in s_funs.iter().zip(&d_funs) {
        let body = match df.body {
            None => None,
            Some(body) => {
                let refs: HashSet<String> = removed
                    .get(&df.name)
                    .map(|v| v.iter().cloned().collect())
                    .unwrap_or_default();
                let mut dm = Demote {
                    cs: &mut cs,
                    promoted: &promoted,
                    removed: &removed,
                    src_sig: &src_sig,
                    refs,
                    locals: HashSet::new(),
                    memo: HashMap::new(),
                };
                Some(dm.body(body))
            }
        };
        rebuilt.push(FirBuilder::new(&mut cs).declare_fun(
            sf.name.clone(),
            sf.typ.clone(),
            &sf.args,
            body,
            sf.is_inline,
        ));
    }
    let (ni, no, name, globals, statics, subs) = dp.header.clone();
    let mut b = FirBuilder::new(&mut cs);
    let functions = b.block(&rebuilt);
    let structure = b.block(&s_struct);
    let sub_items = items(&cs, subs);
    let root = FirBuilder::new(&mut cs).module(
        ni, no, name, structure, globals, functions, statics, &sub_items,
    );
    if root == s {
        Ok(())
    } else {
        let s_bodies = items(&cs, sp.functions);
        let r_bodies = match match_fir(&cs, root) {
            FirMatch::Module { functions, .. } => items(&cs, functions),
            _ => Vec::new(),
        };
        let first = s_funs
            .iter()
            .zip(s_bodies.iter().zip(&r_bodies))
            .find(|(_, (a, b))| a != b)
            .map_or_else(|| "the module".to_owned(), |(f, _)| format!("`{}`", f.name));
        Err(PromotionViolation::Demotion(format!(
            "the demoted output is not the source ({first} differs)"
        )))
    }
}

/// The demotion of one function body (rule 4).
struct Demote<'a> {
    cs: &'a mut FirStore,
    promoted: &'a HashMap<String, FirType>,
    removed: &'a HashMap<String, Vec<String>>,
    src_sig: &'a HashMap<String, Vec<NamedType>>,
    /// The promoted references this function lost.
    refs: HashSet<String>,
    /// The promoted fields demoted to locals of this function.
    locals: HashSet<String>,
    memo: HashMap<FirId, FirId>,
}

impl Demote<'_> {
    fn body(&mut self, body: FirId) -> FirId {
        let mut out = Vec::new();
        for item in items(self.cs, body) {
            if self.refs.is_empty()
                && let FirMatch::StoreVar {
                    name,
                    access: AccessType::Struct,
                    value,
                } = match_fir(self.cs, item)
                && let Some(t) = self.promoted.get(&name)
                && !self.locals.contains(&name)
            {
                let t = t.clone();
                let value = self.node(value);
                self.locals.insert(name.clone());
                // Later accesses of the local differ from earlier field ones.
                self.memo.clear();
                out.push(FirBuilder::new(self.cs).declare_var(
                    name,
                    t,
                    AccessType::Stack,
                    Some(value),
                ));
                continue;
            }
            out.push(self.node(item));
        }
        FirBuilder::new(self.cs).block(&out)
    }

    fn node(&mut self, id: FirId) -> FirId {
        if let Some(&done) = self.memo.get(&id) {
            return done;
        }
        let out = match match_fir(self.cs, id) {
            FirMatch::LoadVar {
                name,
                access: AccessType::Struct,
                typ,
            } if self.refs.contains(&name) => {
                let zero = FirBuilder::new(self.cs).int32(0);
                FirBuilder::new(self.cs).load_table(name, AccessType::FunArgs, zero, typ)
            }
            FirMatch::StoreVar {
                name,
                access: AccessType::Struct,
                value,
            } if self.refs.contains(&name) => {
                let value = self.node(value);
                let zero = FirBuilder::new(self.cs).int32(0);
                FirBuilder::new(self.cs).store_table(name, AccessType::FunArgs, zero, value)
            }
            FirMatch::LoadVar {
                name,
                access: AccessType::Struct,
                typ,
            } if self.locals.contains(&name) => {
                FirBuilder::new(self.cs).load_var(name, AccessType::Stack, typ)
            }
            FirMatch::StoreVar {
                name,
                access: AccessType::Struct,
                value,
            } if self.locals.contains(&name) => {
                let value = self.node(value);
                FirBuilder::new(self.cs).store_var(name, AccessType::Stack, value)
            }
            FirMatch::Drop(value) => match match_fir(self.cs, value) {
                FirMatch::FunCall { name, args, typ } if self.removed.contains_key(&name) => {
                    let gone: HashSet<&String> = self.removed[&name].iter().collect();
                    let sig = self.src_sig.get(&name).cloned().unwrap_or_default();
                    let mut given = args.iter();
                    let mut full = Vec::with_capacity(sig.len());
                    for a in &sig {
                        if gone.contains(&a.name) {
                            let mut b = FirBuilder::new(self.cs);
                            full.push(if self.refs.contains(&a.name) {
                                b.load_var(a.name.clone(), AccessType::FunArgs, a.typ.clone())
                            } else {
                                b.load_var_address(a.name.clone(), AccessType::Stack, a.typ.clone())
                            });
                        } else if let Some(&g) = given.next() {
                            full.push(self.node(g));
                        }
                    }
                    full.extend(given.copied());
                    let mut b = FirBuilder::new(self.cs);
                    let call = b.fun_call(name, &full, typ);
                    b.drop_(call)
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
