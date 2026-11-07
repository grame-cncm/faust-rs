//! Independent checker of the voice layout (design
//! `porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md` §3.3).
//!
//! [`check_voice_layout`] accepts an output of the layout pass only if it is
//! the source with its per-voice state laid out as the design says. It shares
//! no code with the producer beyond the FIR matcher and restates the contract
//! (the added names, the instance functions) on its own:
//!
//! - **classification.** A source `Struct` variable absent from the output's
//!   structure is per-voice; one present must be declared identically and is
//!   shared. A shared field must not be touched by an instance function nor
//!   be a control; a per-voice field must not be touched by `staticInit` or
//!   any function outside the instance lifecycle; a literal `Struct` table
//!   must stay shared. The output structure adds exactly `fVoices: Int32` and
//!   one `Ptr(T)` arena per element type of the per-voice fields.
//! - **layout.** Bases are recomputed from the source: per element type, the
//!   per-voice fields in declaration order, each after the previous one's
//!   length. Disjoint and in bounds by construction, so any other base the
//!   output uses is a violation.
//! - **bodies.** Source and output are walked in lockstep, nodes compared
//!   across the two stores by kind (tags by name). An access to a per-voice
//!   field must correspond to an access to its type's arena at
//!   `(base + i) * fVoices + voice` (`base * fVoices + voice` for a scalar),
//!   `i` the source index unchanged, a stored value cast to the arena type
//!   exactly when its own type differs; everything else must be identical.
//! - **signatures.** The four instance functions, and only they, gain
//!   `voice: Int32` right after `dsp`.
//! - **controls.** `fVoiceControls` holds, in user-interface order, the base
//!   of every control, and is declared only when there is a control.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use tlib::NodeKind;

use crate::{AccessType, FirBinOp, FirId, FirMatch, FirStore, FirType, NamedType, match_fir};

// The contract, restated rather than imported from the producer.
const VOICES: &str = "fVoices";
const VOICE: &str = "voice";
const CONTROLS: &str = "fVoiceControls";
const LIFECYCLE: [&str; 4] = [
    "instanceConstants",
    "instanceResetUserInterface",
    "instanceClear",
    "compute",
];

/// Why [`check_voice_layout`] rejected an output: the rule broken and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutViolation {
    /// A short name of the rule (module doc).
    pub rule: &'static str,
    /// What was found.
    pub detail: String,
}

impl fmt::Display for LayoutViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "voice layout: {}: {}", self.rule, self.detail)
    }
}

impl std::error::Error for LayoutViolation {}

fn violation(rule: &'static str, detail: impl Into<String>) -> LayoutViolation {
    LayoutViolation {
        rule,
        detail: detail.into(),
    }
}

/// Checks that `dst_module` in `dst` is `src_module` in `src` with its
/// per-voice state laid out as a structure of arrays (module doc).
///
/// # Errors
/// The first [`LayoutViolation`] found.
pub fn check_voice_layout(
    src: &FirStore,
    src_module: FirId,
    dst: &FirStore,
    dst_module: FirId,
) -> Result<(), LayoutViolation> {
    tlib::on_compile_stack(|| check_step(src, src_module, dst, dst_module))
}

struct Sections {
    num_inputs: usize,
    num_outputs: usize,
    name: String,
    dsp_struct: Vec<FirId>,
    globals: FirId,
    functions: Vec<FirId>,
    static_decls: Vec<FirId>,
    sub_modules: Vec<FirId>,
}

fn items(store: &FirStore, id: FirId) -> Vec<FirId> {
    match match_fir(store, id) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    }
}

fn sections(store: &FirStore, id: FirId, which: &str) -> Result<Sections, LayoutViolation> {
    let FirMatch::Module {
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        sub_modules,
    } = match_fir(store, id)
    else {
        return Err(violation(
            "module",
            format!("the {which} root is not a module"),
        ));
    };
    Ok(Sections {
        num_inputs,
        num_outputs,
        name,
        dsp_struct: items(store, dsp_struct),
        globals,
        functions: items(store, functions),
        static_decls: items(store, static_decls),
        sub_modules: items(store, sub_modules),
    })
}

/// A per-voice field as the checker places it.
#[derive(Clone)]
struct Slot {
    arena: String,
    elem: FirType,
    base: usize,
}

/// `Struct` names touched, and control zones in pre-order, under `id`.
fn touched(store: &FirStore, id: FirId, names: &mut HashSet<String>, controls: &mut Vec<String>) {
    let mut seen = HashSet::new();
    let mut stack = vec![id];
    // Pre-order with children in order: push them reversed.
    while let Some(node) = stack.pop() {
        if !seen.insert(node) {
            continue;
        }
        match match_fir(store, node) {
            FirMatch::LoadVar {
                name,
                access: AccessType::Struct,
                ..
            }
            | FirMatch::LoadTable {
                name,
                access: AccessType::Struct,
                ..
            }
            | FirMatch::LoadVarAddress {
                name,
                access: AccessType::Struct,
                ..
            }
            | FirMatch::StoreVar {
                name,
                access: AccessType::Struct,
                ..
            }
            | FirMatch::StoreTable {
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
                names.insert(name);
            }
            FirMatch::AddButton { var, .. }
            | FirMatch::AddSlider { var, .. }
            | FirMatch::AddBargraph { var, .. }
                if !controls.contains(&var) =>
            {
                controls.push(var);
            }
            _ => {}
        }
        if let Some(children) = store.arena.children(node) {
            stack.extend(children.iter().rev().copied());
        }
    }
}

struct Checker<'a> {
    src: &'a FirStore,
    dst: &'a FirStore,
    slots: HashMap<String, Slot>,
    /// Pairs already found equal, with the rewriting flag.
    same: HashSet<(FirId, FirId, bool)>,
}

fn check_step(
    src: &FirStore,
    src_module: FirId,
    dst: &FirStore,
    dst_module: FirId,
) -> Result<(), LayoutViolation> {
    let s = sections(src, src_module, "source")?;
    let d = sections(dst, dst_module, "output")?;
    if (s.num_inputs, s.num_outputs, &s.name) != (d.num_inputs, d.num_outputs, &d.name) {
        return Err(violation("module", "inputs, outputs or name changed"));
    }
    if !s.sub_modules.is_empty() || !d.sub_modules.is_empty() {
        return Err(violation("module", "sub-modules present"));
    }

    // The output structure, by name.
    let mut out_decls: BTreeMap<String, FirId> = BTreeMap::new();
    for &decl in &d.dsp_struct {
        let name = match match_fir(dst, decl) {
            FirMatch::DeclareVar { name, .. } | FirMatch::DeclareTable { name, .. } => name,
            _ => {
                return Err(violation(
                    "structure",
                    "a non-declaration in the output structure",
                ));
            }
        };
        if out_decls.insert(name.clone(), decl).is_some() {
            return Err(violation("structure", format!("`{name}` declared twice")));
        }
    }

    let mut checker = Checker {
        src,
        dst,
        slots: HashMap::new(),
        same: HashSet::new(),
    };

    // Classification and layout, from the source declarations.
    let mut shared: HashSet<String> = HashSet::new();
    let mut per_voice_types: Vec<FirType> = Vec::new();
    let mut next: Vec<(FirType, usize)> = Vec::new();
    let mut per_voice: Vec<(String, FirType, usize)> = Vec::new();
    for &decl in &s.dsp_struct {
        match match_fir(src, decl) {
            FirMatch::DeclareVar { name, typ, .. } => {
                if let Some(&out) = out_decls.get(&name) {
                    checker.equal(decl, out)?;
                    shared.insert(name);
                    continue;
                }
                let (elem, len) = match typ {
                    FirType::Array(elem, n) => (*elem, n),
                    other => (other, 1),
                };
                let base = match next.iter_mut().find(|(t, _)| *t == elem) {
                    Some((_, used)) => {
                        let base = *used;
                        *used += len;
                        base
                    }
                    None => {
                        next.push((elem.clone(), len));
                        per_voice_types.push(elem.clone());
                        0
                    }
                };
                per_voice.push((name, elem, base));
            }
            FirMatch::DeclareTable { name, .. } => {
                let Some(&out) = out_decls.get(&name) else {
                    return Err(violation(
                        "shared table",
                        format!("literal table `{name}` left the structure"),
                    ));
                };
                checker.equal(decl, out)?;
                shared.insert(name);
            }
            _ => {}
        }
    }

    // What the output adds: fVoices and one arena per per-voice type.
    let mut arenas: Vec<(FirType, String)> = Vec::new();
    for (name, &decl) in &out_decls {
        if shared.contains(name) {
            continue;
        }
        match match_fir(dst, decl) {
            FirMatch::DeclareVar {
                name,
                typ: FirType::Int32,
                access: AccessType::Struct,
                init: None,
            } if name == VOICES => {}
            FirMatch::DeclareVar {
                name,
                typ: FirType::Ptr(elem),
                access: AccessType::Struct,
                init: None,
            } => {
                if arenas.iter().any(|(t, _)| *t == *elem) {
                    return Err(violation("arenas", format!("two arenas of type {elem:?}")));
                }
                arenas.push((*elem, name));
            }
            _ => {
                return Err(violation(
                    "structure",
                    format!("unexpected output field `{name}`"),
                ));
            }
        }
    }
    if !out_decls.contains_key(VOICES) {
        return Err(violation("structure", format!("no `{VOICES}` field")));
    }
    if arenas.len() != per_voice_types.len()
        || per_voice_types
            .iter()
            .any(|t| !arenas.iter().any(|(a, _)| a == t))
    {
        return Err(violation(
            "arenas",
            "the arenas do not match the per-voice element types",
        ));
    }
    for (name, elem, base) in per_voice {
        let arena = arenas
            .iter()
            .find(|(t, _)| *t == elem)
            .map(|(_, a)| a.clone())
            .unwrap_or_default();
        checker.slots.insert(name, Slot { arena, elem, base });
    }

    // Who touches what, in the source.
    let mut controls = Vec::new();
    for &f in &s.functions {
        let FirMatch::DeclareFun {
            name,
            body: Some(body),
            ..
        } = match_fir(src, f)
        else {
            continue;
        };
        let mut names = HashSet::new();
        touched(src, body, &mut names, &mut controls);
        let instance = LIFECYCLE.contains(&name.as_str());
        for field in &names {
            if instance && shared.contains(field) {
                return Err(violation(
                    "shared use",
                    format!("`{name}` touches shared field `{field}`"),
                ));
            }
            if !instance && checker.slots.contains_key(field) {
                return Err(violation(
                    "class use",
                    format!("`{name}` touches per-voice field `{field}`"),
                ));
            }
        }
    }
    for control in &controls {
        if !checker.slots.contains_key(control) {
            return Err(violation(
                "control",
                format!("control `{control}` is not per-voice"),
            ));
        }
    }

    // Functions, in order.
    if s.functions.len() != d.functions.len() {
        return Err(violation("functions", "the function count changed"));
    }
    for (&sf, &df) in s.functions.iter().zip(&d.functions) {
        checker.function(sf, df)?;
    }

    // Globals unchanged; static declarations kept, plus the control table.
    checker.equal(s.globals, d.globals)?;
    if d.static_decls.len() < s.static_decls.len() {
        return Err(violation("static", "a static declaration was dropped"));
    }
    for (&a, &b) in s.static_decls.iter().zip(&d.static_decls) {
        checker.equal(a, b)?;
    }
    let extra = &d.static_decls[s.static_decls.len()..];
    match (controls.is_empty(), extra) {
        (true, []) => {}
        (false, [table]) => checker.controls_table(*table, &controls)?,
        _ => {
            return Err(violation(
                "controls",
                "the control table is missing or there are extra static declarations",
            ));
        }
    }
    Ok(())
}

impl Checker<'_> {
    fn equal(&mut self, s: FirId, d: FirId) -> Result<(), LayoutViolation> {
        self.lockstep(s, d, false)
    }

    fn function(&mut self, sf: FirId, df: FirId) -> Result<(), LayoutViolation> {
        let FirMatch::DeclareFun {
            name,
            typ,
            args,
            body: Some(body),
            is_inline,
        } = match_fir(self.src, sf)
        else {
            return self.equal(sf, df);
        };
        if !LIFECYCLE.contains(&name.as_str()) {
            if let FirMatch::DeclareFun {
                name: out_name,
                typ: out_typ,
                args: out_args,
                ..
            } = match_fir(self.dst, df)
                && (out_name != name || out_args != args || out_typ != typ)
            {
                return Err(violation(
                    "signature",
                    format!("`{name}` is outside the instance lifecycle and changed signature"),
                ));
            }
            return self.equal(sf, df);
        }
        let FirMatch::DeclareFun {
            name: out_name,
            typ: out_typ,
            args: out_args,
            body: Some(out_body),
            is_inline: out_inline,
        } = match_fir(self.dst, df)
        else {
            return Err(violation("signature", format!("`{name}` lost its body")));
        };
        let mut want_args: Vec<NamedType> = args;
        want_args.insert(
            1.min(want_args.len()),
            NamedType {
                name: VOICE.to_owned(),
                typ: FirType::Int32,
            },
        );
        let want_typ = match typ {
            FirType::Fun { mut args, ret } => {
                args.insert(1.min(args.len()), FirType::Int32);
                FirType::Fun { args, ret }
            }
            other => other,
        };
        if out_name != name
            || out_args != want_args
            || out_typ != want_typ
            || out_inline != is_inline
        {
            return Err(violation(
                "signature",
                format!("`{name}` does not take the voice right after dsp"),
            ));
        }
        self.lockstep(body, out_body, true)
    }

    fn controls_table(&mut self, table: FirId, controls: &[String]) -> Result<(), LayoutViolation> {
        let FirMatch::DeclareTable {
            name,
            access: AccessType::Static,
            elem_type: FirType::Int32,
            values,
        } = match_fir(self.dst, table)
        else {
            return Err(violation(
                "controls",
                "the control table is not a static Int32 table",
            ));
        };
        if name != CONTROLS || values.len() != controls.len() {
            return Err(violation(
                "controls",
                "the control table has the wrong name or size",
            ));
        }
        for (value, control) in values.iter().zip(controls) {
            let base = self.slots.get(control).map(|s| s.base);
            let found = match match_fir(self.dst, *value) {
                FirMatch::Int32 { value, .. } => usize::try_from(value).ok(),
                _ => None,
            };
            if found.is_none() || found != base {
                return Err(violation(
                    "controls",
                    format!("the base of control `{control}` is wrong"),
                ));
            }
        }
        Ok(())
    }

    /// The per-voice slot of a source access, when `rewrite` is on.
    fn slot(&self, name: &str, access: AccessType, rewrite: bool) -> Option<Slot> {
        (rewrite && access == AccessType::Struct)
            .then(|| self.slots.get(name).cloned())
            .flatten()
    }

    fn lockstep(&mut self, s: FirId, d: FirId, rewrite: bool) -> Result<(), LayoutViolation> {
        if !self.same.insert((s, d, rewrite)) {
            return Ok(());
        }
        match match_fir(self.src, s) {
            FirMatch::LoadVar { name, access, typ } => {
                if let Some(slot) = self.slot(&name, access, rewrite) {
                    let FirMatch::LoadTable {
                        name: arena,
                        access: AccessType::Struct,
                        index,
                        typ: out_typ,
                    } = match_fir(self.dst, d)
                    else {
                        return Err(residual(&name));
                    };
                    if arena != slot.arena || out_typ != typ {
                        return Err(residual(&name));
                    }
                    return self.index(&name, index, &slot, None);
                }
            }
            FirMatch::StoreVar {
                name,
                access,
                value,
            } => {
                if let Some(slot) = self.slot(&name, access, rewrite) {
                    let FirMatch::StoreTable {
                        name: arena,
                        access: AccessType::Struct,
                        index,
                        value: out_value,
                    } = match_fir(self.dst, d)
                    else {
                        return Err(residual(&name));
                    };
                    if arena != slot.arena {
                        return Err(residual(&name));
                    }
                    self.index(&name, index, &slot, None)?;
                    return self.stored(&name, value, out_value, &slot);
                }
            }
            FirMatch::LoadTable {
                name,
                access,
                index,
                typ,
            } => {
                if let Some(slot) = self.slot(&name, access, rewrite) {
                    let FirMatch::LoadTable {
                        name: arena,
                        access: AccessType::Struct,
                        index: out_index,
                        typ: out_typ,
                    } = match_fir(self.dst, d)
                    else {
                        return Err(residual(&name));
                    };
                    if arena != slot.arena || out_typ != typ {
                        return Err(residual(&name));
                    }
                    return self.index(&name, out_index, &slot, Some(index));
                }
            }
            FirMatch::StoreTable {
                name,
                access,
                index,
                value,
            } => {
                if let Some(slot) = self.slot(&name, access, rewrite) {
                    let FirMatch::StoreTable {
                        name: arena,
                        access: AccessType::Struct,
                        index: out_index,
                        value: out_value,
                    } = match_fir(self.dst, d)
                    else {
                        return Err(residual(&name));
                    };
                    if arena != slot.arena {
                        return Err(residual(&name));
                    }
                    self.index(&name, out_index, &slot, Some(index))?;
                    return self.stored(&name, value, out_value, &slot);
                }
            }
            _ => {}
        }
        self.same_node(s, d, rewrite)
    }

    /// The same node kind, and the children pairwise equal.
    fn same_node(&mut self, s: FirId, d: FirId, rewrite: bool) -> Result<(), LayoutViolation> {
        let (Some(sk), Some(dk)) = (self.src.arena.kind(s), self.dst.arena.kind(d)) else {
            return Err(violation("body", "a node is missing"));
        };
        let kinds_agree = match (sk, dk) {
            (NodeKind::Tag(a), NodeKind::Tag(b)) => {
                self.src.arena.tag_name(*a) == self.dst.arena.tag_name(*b)
            }
            (a, b) => a == b,
        };
        let sc = self
            .src
            .arena
            .children(s)
            .map(<[FirId]>::to_vec)
            .unwrap_or_default();
        let dc = self
            .dst
            .arena
            .children(d)
            .map(<[FirId]>::to_vec)
            .unwrap_or_default();
        if !kinds_agree || sc.len() != dc.len() {
            return Err(violation(
                "body",
                format!(
                    "the output differs from the source at a {:?} node",
                    match_fir(self.src, s)
                )
                .chars()
                .take(160)
                .collect::<String>(),
            ));
        }
        for (a, b) in sc.into_iter().zip(dc) {
            self.lockstep(a, b, rewrite)?;
        }
        Ok(())
    }

    /// `out` is `(base + i) * fVoices + voice`, or `base * fVoices + voice`
    /// with `src_index` `None`.
    fn index(
        &mut self,
        field: &str,
        out: FirId,
        slot: &Slot,
        src_index: Option<FirId>,
    ) -> Result<(), LayoutViolation> {
        let bad = || {
            violation(
                "index",
                format!("an access to `{field}` is not at its slot"),
            )
        };
        let FirMatch::BinOp {
            op: FirBinOp::Add,
            lhs: scaled,
            rhs: voice,
            typ: FirType::Int32,
        } = match_fir(self.dst, out)
        else {
            return Err(bad());
        };
        if !matches!(match_fir(self.dst, voice), FirMatch::LoadVar { name, access: AccessType::FunArgs, typ: FirType::Int32 } if name == VOICE)
        {
            return Err(bad());
        }
        let FirMatch::BinOp {
            op: FirBinOp::Mul,
            lhs: row,
            rhs: voices,
            typ: FirType::Int32,
        } = match_fir(self.dst, scaled)
        else {
            return Err(bad());
        };
        if !matches!(match_fir(self.dst, voices), FirMatch::LoadVar { name, access: AccessType::Struct, typ: FirType::Int32 } if name == VOICES)
        {
            return Err(bad());
        }
        let base_is = |store: &FirStore, id: FirId| matches!(match_fir(store, id), FirMatch::Int32 { value, .. } if usize::try_from(value).ok() == Some(slot.base));
        match src_index {
            None => {
                if base_is(self.dst, row) {
                    Ok(())
                } else {
                    Err(bad())
                }
            }
            Some(i) => {
                let FirMatch::BinOp {
                    op: FirBinOp::Add,
                    lhs: base,
                    rhs: out_i,
                    typ: FirType::Int32,
                } = match_fir(self.dst, row)
                else {
                    return Err(bad());
                };
                if !base_is(self.dst, base) {
                    return Err(bad());
                }
                self.lockstep(i, out_i, true)
            }
        }
    }

    /// The stored value: the source value, cast to the arena type exactly
    /// when its own type differs.
    fn stored(
        &mut self,
        field: &str,
        value: FirId,
        out: FirId,
        slot: &Slot,
    ) -> Result<(), LayoutViolation> {
        match self.src.value_type(value) {
            Some(t) if t != slot.elem => {
                let FirMatch::Cast { typ, value: inner } = match_fir(self.dst, out) else {
                    return Err(violation(
                        "store",
                        format!("a store to `{field}` of another type is not cast"),
                    ));
                };
                if typ != slot.elem {
                    return Err(violation(
                        "store",
                        format!("a store to `{field}` is cast to the wrong type"),
                    ));
                }
                self.lockstep(value, inner, true)
            }
            _ => self.lockstep(value, out, true),
        }
    }
}

fn residual(field: &str) -> LayoutViolation {
    violation(
        "residual",
        format!("an access to per-voice field `{field}` is not on its arena"),
    )
}
