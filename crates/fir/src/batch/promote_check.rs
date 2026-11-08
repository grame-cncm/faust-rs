//! Independent checker of the state promotion (plan
//! `porting/batch-state-register-promotion-plan-2026-11-08-en.md` §3).
//!
//! [`check_promotion`] accepts an output of [`super::promote_state`] only if
//! it is the source with some `compute` state moved to `Stack` locals as the
//! plan says. It shares no code with the producer beyond the FIR matcher and
//! restates the contract (the local names, the function rewritten) on its
//! own. Both modules are imported into one checking store, where identical
//! trees intern to one id, so "unchanged" is an id comparison. The rules, in
//! the order they are checked:
//!
//! - **sections.** Everything but the `compute` body is unchanged: the module
//!   header, the structure, globals, static declarations, sub-modules, every
//!   other function, and `compute`'s signature.
//! - **entry-exit.** The output body is `entry ++ middle ++ exit`. `entry` is
//!   the run of `Stack` declarations named `<field>_reg` (initialised from
//!   the scalar `Struct` field) or `<field>_reg<k>` (initialised from element
//!   `k`, an `Int32` literal), each declared once. `exit` is the run of
//!   `Struct` stores of an entry local into the element it was read from,
//!   each at most once.
//! - **early-exit.** When anything is promoted, the source `compute` has no
//!   `Return` and calls no function defined in the module.
//! - **literal-only.** In the source `compute`, a field with a promoted
//!   element is accessed only as a scalar or at literal indices, never
//!   through its address, a `TeeVar` or a shift.
//! - **memory-access.** `middle` reaches no promoted field through `Struct`
//!   memory.
//! - **unpromote.** Replacing in `middle` every load of an entry local by the
//!   access it was initialised from, and every store to it by a store to that
//!   element, gives the source body.
//! - **write-back.** An element is in `exit` exactly when the source
//!   `compute` stores into it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

use crate::{AccessType, FirBuilder, FirId, FirMatch, FirStore, FirType, match_fir};

// The contract, restated rather than imported from the producer.
const COMPUTE: &str = "compute";

fn local_for(field: &str, index: Option<i32>) -> String {
    match index {
        None => format!("{field}_reg"),
        Some(k) => format!("{field}_reg{k}"),
    }
}

/// Why [`check_promotion`] rejected an output: the rule broken and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionViolation {
    /// The rule's short name (module doc).
    pub rule: &'static str,
    /// What was found.
    pub detail: String,
}

impl fmt::Display for PromotionViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "state promotion: {}: {}", self.rule, self.detail)
    }
}

impl std::error::Error for PromotionViolation {}

fn violation(rule: &'static str, detail: impl Into<String>) -> PromotionViolation {
    PromotionViolation {
        rule,
        detail: detail.into(),
    }
}

fn items(store: &FirStore, id: FirId) -> Vec<FirId> {
    match match_fir(store, id) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    }
}

/// A promoted element: its field and index (`None` for a scalar), its type.
#[derive(Clone)]
struct Element {
    field: String,
    index: Option<i32>,
    typ: FirType,
}

fn int_literal(store: &FirStore, id: FirId) -> Option<i32> {
    match match_fir(store, id) {
        FirMatch::Int32 { value, .. } => Some(value),
        _ => None,
    }
}

/// The `Struct` accesses under `id`: scalar or literal ones as (field, index
/// or `None`, stored), the others by field; and whether a `Return` or calls
/// appear.
#[derive(Default)]
struct Accesses {
    literal: BTreeSet<(String, Option<i32>, bool)>,
    other: BTreeSet<String>,
    returns: bool,
    calls: BTreeSet<String>,
}

fn accesses(store: &FirStore, id: FirId, seen: &mut HashSet<FirId>, out: &mut Accesses) {
    if !seen.insert(id) {
        return;
    }
    match match_fir(store, id) {
        FirMatch::LoadVar {
            name,
            access: AccessType::Struct,
            ..
        } => {
            out.literal.insert((name, None, false));
        }
        FirMatch::StoreVar {
            name,
            access: AccessType::Struct,
            ..
        } => {
            out.literal.insert((name, None, true));
        }
        FirMatch::LoadTable {
            name,
            access: AccessType::Struct,
            index,
            ..
        } => match int_literal(store, index) {
            Some(k) => {
                out.literal.insert((name, Some(k), false));
            }
            None => {
                out.other.insert(name);
            }
        },
        FirMatch::StoreTable {
            name,
            access: AccessType::Struct,
            index,
            ..
        } => match int_literal(store, index) {
            Some(k) => {
                out.literal.insert((name, Some(k), true));
            }
            None => {
                out.other.insert(name);
            }
        },
        FirMatch::LoadVarAddress {
            name,
            access: AccessType::Struct,
            ..
        }
        | FirMatch::TeeVar {
            name,
            access: AccessType::Struct,
            ..
        }
        | FirMatch::ShiftArrayVar {
            name,
            access: AccessType::Struct,
            ..
        } => {
            out.other.insert(name);
        }
        FirMatch::Return(_) => out.returns = true,
        FirMatch::FunCall { name, .. } => {
            out.calls.insert(name);
        }
        _ => {}
    }
    if let Some(children) = store.arena.children(id) {
        for &child in children {
            accesses(store, child, seen, out);
        }
    }
}

/// The parts of a module the checker compares.
struct Parts {
    header: (usize, usize, String),
    sections: [FirId; 4],
    functions: Vec<FirId>,
}

fn parts(store: &FirStore, root: FirId, which: &str) -> Result<Parts, PromotionViolation> {
    let FirMatch::Module {
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        sub_modules,
    } = match_fir(store, root)
    else {
        return Err(violation(
            "sections",
            format!("the {which} root is not a module"),
        ));
    };
    Ok(Parts {
        header: (num_inputs, num_outputs, name),
        sections: [dsp_struct, globals, static_decls, sub_modules],
        functions: items(store, functions),
    })
}

/// Checks that `dst_module` in `dst` is `src_module` in `src` with some
/// `compute` state promoted to locals (module doc).
///
/// # Errors
/// The first [`PromotionViolation`] found.
pub fn check_promotion(
    src: &FirStore,
    src_module: FirId,
    dst: &FirStore,
    dst_module: FirId,
) -> Result<(), PromotionViolation> {
    let mut cs = FirStore::new();
    let s = cs.import_from(src, src_module);
    let d = cs.import_from(dst, dst_module);
    let sp = parts(&cs, s, "source")?;
    let dp = parts(&cs, d, "output")?;

    // sections
    if sp.header != dp.header || sp.sections != dp.sections {
        return Err(violation(
            "sections",
            "the module header or a section other than the functions changed",
        ));
    }
    if sp.functions.len() != dp.functions.len() {
        return Err(violation("sections", "the number of functions changed"));
    }
    let mut defined = BTreeSet::new();
    let mut pair = None;
    for (&sf, &df) in sp.functions.iter().zip(&dp.functions) {
        let FirMatch::DeclareFun {
            name,
            typ,
            args,
            body,
            is_inline,
        } = match_fir(&cs, sf)
        else {
            if sf != df {
                return Err(violation("sections", "a function declaration changed"));
            }
            continue;
        };
        if body.is_some() {
            defined.insert(name.clone());
        }
        if name != COMPUTE || body.is_none() {
            if sf != df {
                return Err(violation("sections", format!("function `{name}` changed")));
            }
            continue;
        }
        let FirMatch::DeclareFun {
            name: dname,
            typ: dtyp,
            args: dargs,
            body: Some(dbody),
            is_inline: dinline,
        } = match_fir(&cs, df)
        else {
            return Err(violation("sections", "`compute` lost its body"));
        };
        if dname != name || dtyp != typ || dargs != args || dinline != is_inline {
            return Err(violation("sections", "the signature of `compute` changed"));
        }
        pair = body.map(|b| (b, dbody));
    }
    let Some((sbody, dbody)) = pair else {
        return if s == d {
            Ok(())
        } else {
            Err(violation(
                "sections",
                "no `compute`, yet the module changed",
            ))
        };
    };

    // entry-exit
    let ditems = items(&cs, dbody);
    let mut locals: BTreeMap<String, Element> = BTreeMap::new();
    let mut at = 0;
    while let Some(&item) = ditems.get(at) {
        let FirMatch::DeclareVar {
            name,
            typ,
            access: AccessType::Stack,
            init: Some(init),
        } = match_fir(&cs, item)
        else {
            break;
        };
        let element = match match_fir(&cs, init) {
            FirMatch::LoadVar {
                name: field,
                access: AccessType::Struct,
                typ: t,
            } if t == typ => Element {
                field,
                index: None,
                typ,
            },
            FirMatch::LoadTable {
                name: field,
                access: AccessType::Struct,
                index,
                typ: t,
            } if t == typ => match int_literal(&cs, index) {
                Some(k) => Element {
                    field,
                    index: Some(k),
                    typ,
                },
                None => break,
            },
            _ => break,
        };
        if name != local_for(&element.field, element.index) {
            // A declaration of the source ends the entry run; one named as
            // the local of another element of the same field is a defect.
            if name.starts_with(&format!("{}_reg", element.field)) {
                return Err(violation(
                    "entry-exit",
                    format!(
                        "local `{name}` is initialised from `{}`{}, which is not the element its name says",
                        element.field,
                        element.index.map_or(String::new(), |k| format!("[{k}]"))
                    ),
                ));
            }
            break;
        }
        if locals.insert(name.clone(), element).is_some() {
            return Err(violation(
                "entry-exit",
                format!("local `{name}` declared twice"),
            ));
        }
        at += 1;
    }
    let entry_end = at;
    let mut exit_start = ditems.len();
    let mut written: BTreeSet<String> = BTreeSet::new();
    while exit_start > entry_end {
        let item = ditems[exit_start - 1];
        let (field, index, value) = match match_fir(&cs, item) {
            FirMatch::StoreVar {
                name,
                access: AccessType::Struct,
                value,
            } => (name, None, value),
            FirMatch::StoreTable {
                name,
                access: AccessType::Struct,
                index,
                value,
            } => match int_literal(&cs, index) {
                Some(k) => (name, Some(k), value),
                None => break,
            },
            _ => break,
        };
        let FirMatch::LoadVar {
            name: local,
            access: AccessType::Stack,
            ..
        } = match_fir(&cs, value)
        else {
            break;
        };
        let Some(e) = locals.get(&local) else {
            break;
        };
        if e.field != field || e.index != index {
            return Err(violation(
                "entry-exit",
                format!("local `{local}` is written back to another element of `{field}`"),
            ));
        }
        if !written.insert(local.clone()) {
            return Err(violation(
                "entry-exit",
                format!("local `{local}` written back twice"),
            ));
        }
        exit_start -= 1;
    }
    if locals.is_empty() {
        return if sbody == dbody {
            Ok(())
        } else {
            Err(violation(
                "unpromote",
                "nothing is promoted, yet `compute` changed",
            ))
        };
    }

    // early-exit
    let mut src_acc = Accesses::default();
    accesses(&cs, sbody, &mut HashSet::new(), &mut src_acc);
    if src_acc.returns {
        return Err(violation(
            "early-exit",
            "the source `compute` returns early",
        ));
    }
    if let Some(call) = src_acc.calls.iter().find(|c| defined.contains(*c)) {
        return Err(violation(
            "early-exit",
            format!("the source `compute` calls `{call}`, a function of the module"),
        ));
    }

    // literal-only
    let promoted_fields: BTreeSet<&str> = locals.values().map(|e| e.field.as_str()).collect();
    if let Some(field) = src_acc
        .other
        .iter()
        .find(|f| promoted_fields.contains(f.as_str()))
    {
        return Err(violation(
            "literal-only",
            format!(
                "`{field}` is promoted but the source reaches it at a dynamic index, by address, a tee or a shift"
            ),
        ));
    }

    // memory-access
    let middle = &ditems[entry_end..exit_start];
    let mut mid_acc = Accesses::default();
    let mut seen = HashSet::new();
    for &m in middle {
        accesses(&cs, m, &mut seen, &mut mid_acc);
    }
    let touched = mid_acc
        .literal
        .iter()
        .map(|(f, _, _)| f)
        .chain(&mid_acc.other)
        .find(|f| promoted_fields.contains(f.as_str()));
    if let Some(field) = touched {
        return Err(violation(
            "memory-access",
            format!("`{field}` is promoted but the body still reaches it in memory"),
        ));
    }

    // unpromote
    let by_local: HashMap<String, Element> = locals.clone().into_iter().collect();
    let mut undo = Unpromote {
        store: &mut cs,
        locals: &by_local,
        memo: HashMap::new(),
    };
    let restored: Vec<FirId> = middle.iter().map(|&m| undo.rewrite(m)).collect();
    let restored = FirBuilder::new(&mut cs).block(&restored);
    if restored != sbody {
        return Err(violation(
            "unpromote",
            "the body with its locals put back on their fields is not the source body",
        ));
    }

    // write-back
    let stored: BTreeSet<String> = src_acc
        .literal
        .iter()
        .filter(|(_, _, st)| *st)
        .map(|(f, k, _)| local_for(f, *k))
        .filter(|l| locals.contains_key(l))
        .collect();
    if let Some(lost) = stored.difference(&written).next() {
        return Err(violation(
            "write-back",
            format!("`compute` stores into the element of `{lost}`, which is not written back"),
        ));
    }
    if let Some(extra) = written.difference(&stored).next() {
        return Err(violation(
            "write-back",
            format!("`{extra}` is written back, but `compute` never stores into its element"),
        ));
    }
    Ok(())
}

/// Puts the entry locals back on their fields.
struct Unpromote<'a> {
    store: &'a mut FirStore,
    locals: &'a HashMap<String, Element>,
    memo: HashMap<FirId, FirId>,
}

impl Unpromote<'_> {
    fn rewrite(&mut self, id: FirId) -> FirId {
        if let Some(&done) = self.memo.get(&id) {
            return done;
        }
        let out = match match_fir(self.store, id) {
            FirMatch::LoadVar {
                name,
                access: AccessType::Stack,
                ..
            } if self.locals.contains_key(&name) => {
                let e = self.locals[&name].clone();
                let mut b = FirBuilder::new(self.store);
                match e.index {
                    None => b.load_var(e.field, AccessType::Struct, e.typ),
                    Some(k) => {
                        let i = b.int32(k);
                        b.load_table(e.field, AccessType::Struct, i, e.typ)
                    }
                }
            }
            FirMatch::StoreVar {
                name,
                access: AccessType::Stack,
                value,
            } if self.locals.contains_key(&name) => {
                let e = self.locals[&name].clone();
                let value = self.rewrite(value);
                let mut b = FirBuilder::new(self.store);
                match e.index {
                    None => b.store_var(e.field, AccessType::Struct, value),
                    Some(k) => {
                        let i = b.int32(k);
                        b.store_table(e.field, AccessType::Struct, i, value)
                    }
                }
            }
            _ => self.rebuild(id),
        };
        self.memo.insert(id, out);
        out
    }

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
