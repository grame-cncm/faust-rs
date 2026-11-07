//! Per-voice state as a structure of arrays: the layout pass of G3 of the
//! GPU batch plan.
//!
//! Design: `porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`
//! (§3.1 layout, §3.2 pass; decisions §6). Parent plan:
//! `porting/mojo-gpu-batch-of-voices-plan-2026-11-07-en.md`.
//!
//! [`layout_voices`] rewrites a flattened module (no sub-modules) so that one
//! instance structure serves N voices, N known at run time:
//!
//! - every `Struct` field touched by the instance lifecycle
//!   (`instanceConstants`, `instanceResetUserInterface`, `instanceClear`,
//!   `compute`) or named by a control moves into an *arena*, one per element
//!   type: element `k` of field `f` of voice `v` is `arena[(b_f + k) * N + v]`,
//!   so the voices of one element are contiguous (coalesced on a GPU);
//! - the arenas become `Struct` pointer fields (`fState<Type>: Ptr(T)`) beside
//!   `fVoices: Int32` (N);
//! - the four instance functions gain a `voice: Int32` argument, after `dsp`;
//! - `Struct` fields touched only by `staticInit` (the working state of an
//!   inlined table generator) stay shared, as do `Static` and `Global` state
//!   and literal `Struct` tables that are only read;
//! - the arena base of every control, in user-interface order, is declared as
//!   the `Static` table `fVoiceControls`, since UI nodes name their zone.
//!
//! What the pass does not handle it refuses ([`BatchError`]) rather than
//! passes through. Indices are `Int32`: a host must keep every
//! `elements * N` below 2^31.
//!
//! # Public API mapping status
//! - `layout_voices`, `VoiceLayout`, `BatchError`: no C++ counterpart
//!   (Rust-only, `adapted`); C++ Faust has no multi-voice state layout.

#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use crate::{
    AccessType, FirBinOp, FirBuilder, FirId, FirMatch, FirStore, FirType, NamedType, match_fir,
};

/// The `Struct` field holding the voice count N.
pub const VOICES_FIELD: &str = "fVoices";
/// The argument added to the instance functions.
pub const VOICE_ARG: &str = "voice";
/// The `Static` table of control bases, in user-interface order.
pub const CONTROLS_TABLE: &str = "fVoiceControls";
/// The functions that gain the voice argument and see per-voice state.
pub const INSTANCE_FUNCTIONS: [&str; 4] = [
    "instanceConstants",
    "instanceResetUserInterface",
    "instanceClear",
    "compute",
];
/// The class-level initialiser: no voice, shared state only.
const SHARED_FUNCTION: &str = "staticInit";

/// Where one field of a voice lives.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    /// The field's name in the source module.
    pub field: String,
    /// The arena holding it.
    pub arena: String,
    /// The element type (the field's type, or its array element type).
    pub elem_type: FirType,
    /// Its first element in the arena, per voice.
    pub base: usize,
    /// Its number of elements: 1 for a scalar, `n` for `Array(T, n)`.
    pub len: usize,
}

/// One arena: the state of one element type for all voices.
#[derive(Debug, Clone, PartialEq)]
pub struct Arena {
    /// The `Struct` pointer field naming it in the output.
    pub name: String,
    /// Its element type.
    pub elem_type: FirType,
    /// Elements per voice; a host allocates `elements * N`.
    pub elements: usize,
}

/// The layout [`layout_voices`] chose.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VoiceLayout {
    /// Per-voice fields, in declaration order.
    pub placements: Vec<Placement>,
    /// Arenas, in order of first use.
    pub arenas: Vec<Arena>,
    /// `Struct` declarations left shared by all voices.
    pub shared: Vec<String>,
    /// Control fields, in user-interface order (the order of `fVoiceControls`).
    pub controls: Vec<String>,
}

impl VoiceLayout {
    /// The placement of `field`, if it is per-voice state.
    #[must_use]
    pub fn placement(&self, field: &str) -> Option<&Placement> {
        self.placements.iter().find(|p| p.field == field)
    }
}

/// Why [`layout_voices`] refused a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchError {
    /// The root is not a module.
    NotAModule,
    /// Table generators were not flattened first.
    SubModules,
    /// A `Struct` declaration with an initialiser.
    FieldInit(String),
    /// A per-voice field of a type no arena holds.
    UnsupportedFieldType { field: String, typ: String },
    /// A field touched both by `staticInit` and by the instance lifecycle.
    SharedAndInstance(String),
    /// An access to per-voice state the pass cannot rewrite.
    UnsupportedAccess { field: String, what: &'static str },
    /// A function other than the lifecycle touching per-voice state.
    UnsupportedFunction { function: String, field: String },
    /// A soundfile: its zone is not plain state.
    Soundfile,
    /// A name the pass adds already exists.
    NameTaken(String),
    /// An element index that is not `Int32`.
    IndexType(String),
}

impl fmt::Display for BatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAModule => write!(f, "the root is not a FIR module"),
            Self::SubModules => write!(f, "table generators must be flattened first"),
            Self::FieldInit(field) => write!(f, "field `{field}` has an initialiser"),
            Self::UnsupportedFieldType { field, typ } => {
                write!(
                    f,
                    "field `{field}` has type {typ}, which no voice arena holds"
                )
            }
            Self::SharedAndInstance(field) => write!(
                f,
                "field `{field}` is touched by staticInit and by the instance lifecycle"
            ),
            Self::UnsupportedAccess { field, what } => {
                write!(f, "{what} of per-voice field `{field}` is not supported")
            }
            Self::UnsupportedFunction { function, field } => write!(
                f,
                "function `{function}` touches per-voice field `{field}` outside the lifecycle"
            ),
            Self::Soundfile => write!(f, "soundfiles are not supported in the voice layout"),
            Self::NameTaken(name) => write!(f, "`{name}` already exists in the module"),
            Self::IndexType(field) => write!(f, "an index of `{field}` is not Int32"),
        }
    }
}

impl std::error::Error for BatchError {}

/// The arena field of element type `typ`, or `None` when no arena holds it.
#[must_use]
pub fn arena_name(typ: &FirType) -> Option<&'static str> {
    match typ {
        FirType::Int32 => Some("fStateInt32"),
        FirType::Int64 => Some("fStateInt64"),
        FirType::Float32 => Some("fStateFloat32"),
        FirType::Float64 => Some("fStateFloat64"),
        FirType::FaustFloat => Some("fStateFaustFloat"),
        FirType::Bool => Some("fStateBool"),
        _ => None,
    }
}

/// Lays out the per-voice state of `module` as a structure of arrays, in a
/// copy of `src` (module doc). Returns the new store, the new module root and
/// the layout.
///
/// # Errors
/// [`BatchError`] for a module the pass does not handle; nothing is partly
/// rewritten.
pub fn layout_voices(
    src: &FirStore,
    module: FirId,
) -> Result<(FirStore, FirId, VoiceLayout), BatchError> {
    tlib::on_compile_stack(|| layout_voices_step(src, module))
}

/// The pieces of a module the pass reads.
struct ModuleParts {
    num_inputs: usize,
    num_outputs: usize,
    name: String,
    dsp_struct: Vec<FirId>,
    globals: FirId,
    functions: Vec<FirId>,
    static_decls: Vec<FirId>,
}

fn block_items(store: &FirStore, id: FirId) -> Vec<FirId> {
    match match_fir(store, id) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    }
}

fn module_parts(store: &FirStore, root: FirId) -> Result<ModuleParts, BatchError> {
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
        return Err(BatchError::NotAModule);
    };
    if !block_items(store, sub_modules).is_empty() {
        return Err(BatchError::SubModules);
    }
    Ok(ModuleParts {
        num_inputs,
        num_outputs,
        name,
        dsp_struct: block_items(store, dsp_struct),
        globals,
        functions: block_items(store, functions),
        static_decls: block_items(store, static_decls),
    })
}

/// What one function does with `Struct` state.
#[derive(Default)]
struct Touches {
    /// Fields read or written, by name.
    fields: BTreeSet<String>,
    /// Fields stored into.
    stored: BTreeSet<String>,
    /// Control zones, in order.
    controls: Vec<String>,
    soundfile: bool,
}

/// Collects the `Struct` accesses and control zones reachable from `id`.
fn collect(store: &FirStore, id: FirId, seen: &mut BTreeSet<FirId>, out: &mut Touches) {
    if !seen.insert(id) {
        return;
    }
    match match_fir(store, id) {
        FirMatch::LoadVar {
            name,
            access: AccessType::Struct,
            ..
        }
        | FirMatch::LoadVarAddress {
            name,
            access: AccessType::Struct,
            ..
        } => {
            out.fields.insert(name);
        }
        FirMatch::LoadTable {
            name,
            access: AccessType::Struct,
            ..
        } => {
            out.fields.insert(name);
        }
        FirMatch::StoreVar {
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
            out.fields.insert(name.clone());
            out.stored.insert(name);
        }
        FirMatch::AddButton { var, .. }
        | FirMatch::AddSlider { var, .. }
        | FirMatch::AddBargraph { var, .. } => {
            if !out.controls.contains(&var) {
                out.controls.push(var);
            }
        }
        FirMatch::AddSoundfile { .. }
        | FirMatch::LoadSoundfileLength { .. }
        | FirMatch::LoadSoundfileRate { .. }
        | FirMatch::LoadSoundfileChannels { .. }
        | FirMatch::LoadSoundfileBuffer { .. } => out.soundfile = true,
        _ => {}
    }
    if let Some(children) = store.arena.children(id) {
        for &child in children {
            collect(store, child, seen, out);
        }
    }
}

/// One `Struct` declaration of the source.
struct Field {
    name: String,
    decl: FirId,
    /// `(element type, length)` for a `DeclareVar`; `None` for a literal table.
    shape: Option<(FirType, usize)>,
}

fn struct_fields(store: &FirStore, items: &[FirId]) -> Result<Vec<Field>, BatchError> {
    let mut fields = Vec::with_capacity(items.len());
    for &decl in items {
        match match_fir(store, decl) {
            FirMatch::DeclareVar {
                name, typ, init, ..
            } => {
                if init.is_some() {
                    return Err(BatchError::FieldInit(name));
                }
                let shape = match typ {
                    FirType::Array(elem, n) => (*elem, n),
                    other => (other, 1),
                };
                fields.push(Field {
                    name,
                    decl,
                    shape: Some(shape),
                });
            }
            FirMatch::DeclareTable { name, .. } => fields.push(Field {
                name,
                decl,
                shape: None,
            }),
            _ => {}
        }
    }
    Ok(fields)
}

/// The names declared anywhere in the module's sections.
fn declared_names(store: &FirStore, parts: &ModuleParts) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let globals = block_items(store, parts.globals);
    for &id in parts
        .dsp_struct
        .iter()
        .chain(&globals)
        .chain(&parts.static_decls)
    {
        match match_fir(store, id) {
            FirMatch::DeclareVar { name, .. } | FirMatch::DeclareTable { name, .. } => {
                names.insert(name);
            }
            _ => {}
        }
    }
    names
}

fn layout_voices_step(
    src: &FirStore,
    module: FirId,
) -> Result<(FirStore, FirId, VoiceLayout), BatchError> {
    let mut store = FirStore::new();
    let root = store.import_from(src, module);
    let parts = module_parts(&store, root)?;
    let fields = struct_fields(&store, &parts.dsp_struct)?;

    // What every function touches.
    let mut instance = Touches::default();
    let mut shared_touch = Touches::default();
    let mut ui = Touches::default();
    let mut others: Vec<(String, Touches)> = Vec::new();
    for &f in &parts.functions {
        let FirMatch::DeclareFun {
            name,
            body: Some(body),
            ..
        } = match_fir(&store, f)
        else {
            continue;
        };
        let mut t = Touches::default();
        collect(&store, body, &mut BTreeSet::new(), &mut t);
        if t.soundfile {
            return Err(BatchError::Soundfile);
        }
        if INSTANCE_FUNCTIONS.contains(&name.as_str()) {
            instance.fields.extend(t.fields);
            instance.stored.extend(t.stored);
        } else if name == SHARED_FUNCTION {
            shared_touch.fields.extend(t.fields);
            shared_touch.stored.extend(t.stored);
        } else {
            ui.controls.extend(t.controls.iter().cloned());
            others.push((name, t));
        }
    }
    let controls: Vec<String> = {
        let mut seen = BTreeSet::new();
        ui.controls
            .into_iter()
            .filter(|c| seen.insert(c.clone()))
            .collect()
    };

    // Classify: per-voice unless touched by staticInit alone; literal
    // tables stay shared and must not be written.
    let mut layout = VoiceLayout::default();
    let mut placed: HashMap<String, (String, FirType, usize)> = HashMap::new();
    let mut shared_decls = Vec::new();
    for field in &fields {
        let by_instance = instance.fields.contains(&field.name) || controls.contains(&field.name);
        let by_shared = shared_touch.fields.contains(&field.name);
        if by_instance && by_shared {
            return Err(BatchError::SharedAndInstance(field.name.clone()));
        }
        let Some((elem, len)) = &field.shape else {
            if instance.stored.contains(&field.name) || shared_touch.stored.contains(&field.name) {
                return Err(BatchError::UnsupportedAccess {
                    field: field.name.clone(),
                    what: "a store into a literal table",
                });
            }
            layout.shared.push(field.name.clone());
            shared_decls.push(field.decl);
            continue;
        };
        if by_shared {
            layout.shared.push(field.name.clone());
            shared_decls.push(field.decl);
            continue;
        }
        let Some(arena) = arena_name(elem) else {
            return Err(BatchError::UnsupportedFieldType {
                field: field.name.clone(),
                typ: format!("{elem:?}"),
            });
        };
        let at = match layout.arenas.iter().position(|a| a.name == arena) {
            Some(at) => at,
            None => {
                layout.arenas.push(Arena {
                    name: arena.to_owned(),
                    elem_type: elem.clone(),
                    elements: 0,
                });
                layout.arenas.len() - 1
            }
        };
        let base = layout.arenas[at].elements;
        layout.arenas[at].elements += len;
        layout.placements.push(Placement {
            field: field.name.clone(),
            arena: arena.to_owned(),
            elem_type: elem.clone(),
            base,
            len: *len,
        });
        placed.insert(field.name.clone(), (arena.to_owned(), elem.clone(), base));
    }
    for (function, t) in &others {
        if let Some(field) = t.fields.iter().find(|f| placed.contains_key(*f)) {
            return Err(BatchError::UnsupportedFunction {
                function: function.clone(),
                field: field.clone(),
            });
        }
    }
    for control in &controls {
        if !placed.contains_key(control) {
            return Err(BatchError::UnsupportedAccess {
                field: control.clone(),
                what: "a control zone that is not per-voice state",
            });
        }
    }
    layout.controls = controls;

    // The names the pass adds must be free.
    let declared = declared_names(&store, &parts);
    let added = layout
        .arenas
        .iter()
        .map(|a| a.name.as_str())
        .chain([VOICES_FIELD, CONTROLS_TABLE]);
    for name in added {
        if declared.contains(name) {
            return Err(BatchError::NameTaken(name.to_owned()));
        }
    }

    // Rewrite the instance functions.
    let mut rewriter = Rewriter::new(&mut store, &placed);
    let mut functions = Vec::with_capacity(parts.functions.len());
    for &f in &parts.functions {
        functions.push(rewriter.function(f)?);
    }

    // Rebuild the sections.
    let mut b = FirBuilder::new(&mut store);
    let mut dsp_struct = shared_decls;
    dsp_struct.push(b.declare_var(VOICES_FIELD, FirType::Int32, AccessType::Struct, None));
    for arena in &layout.arenas {
        dsp_struct.push(b.declare_var(
            arena.name.clone(),
            FirType::Ptr(Box::new(arena.elem_type.clone())),
            AccessType::Struct,
            None,
        ));
    }
    let mut static_decls = parts.static_decls.clone();
    if !layout.controls.is_empty() {
        let values: Vec<FirId> = layout
            .controls
            .iter()
            .map(|c| {
                let base = layout.placement(c).map_or(0, |p| p.base);
                b.int32(i32::try_from(base).unwrap_or(i32::MAX))
            })
            .collect();
        static_decls.push(b.declare_table(
            CONTROLS_TABLE,
            AccessType::Static,
            FirType::Int32,
            &values,
        ));
    }
    let dsp_struct = b.block(&dsp_struct);
    let functions = b.block(&functions);
    let static_decls = b.block(&static_decls);
    let root = b.module(
        parts.num_inputs,
        parts.num_outputs,
        parts.name,
        dsp_struct,
        parts.globals,
        functions,
        static_decls,
        &[],
    );
    Ok((store, root, layout))
}

/// Rewrites accesses to per-voice fields (design §3.2, step 3).
struct Rewriter<'a> {
    store: &'a mut FirStore,
    placed: &'a HashMap<String, (String, FirType, usize)>,
    memo: HashMap<FirId, FirId>,
}

impl<'a> Rewriter<'a> {
    fn new(store: &'a mut FirStore, placed: &'a HashMap<String, (String, FirType, usize)>) -> Self {
        Self {
            store,
            placed,
            memo: HashMap::new(),
        }
    }

    /// An instance function with its body rewritten and the voice argument
    /// added after `dsp`; any other declaration unchanged.
    fn function(&mut self, f: FirId) -> Result<FirId, BatchError> {
        let FirMatch::DeclareFun {
            name,
            typ,
            args,
            body: Some(body),
            is_inline,
        } = match_fir(self.store, f)
        else {
            return Ok(f);
        };
        if !INSTANCE_FUNCTIONS.contains(&name.as_str()) {
            return Ok(f);
        }
        if args.iter().any(|a| a.name == VOICE_ARG) {
            return Err(BatchError::NameTaken(VOICE_ARG.to_owned()));
        }
        let body = self.rewrite(body)?;
        let at = 1.min(args.len());
        let mut args = args;
        args.insert(
            at,
            NamedType {
                name: VOICE_ARG.to_owned(),
                typ: FirType::Int32,
            },
        );
        let typ = match typ {
            FirType::Fun {
                args: mut arg_types,
                ret,
            } => {
                arg_types.insert(1.min(arg_types.len()), FirType::Int32);
                FirType::Fun {
                    args: arg_types,
                    ret,
                }
            }
            other => other,
        };
        Ok(FirBuilder::new(self.store).declare_fun(name, typ, &args, Some(body), is_inline))
    }

    /// `(b + i) * N + voice`, or `b * N + voice` for a scalar (`index` `None`).
    fn index(&mut self, base: usize, index: Option<FirId>) -> FirId {
        let mut b = FirBuilder::new(self.store);
        let base = b.int32(i32::try_from(base).unwrap_or(i32::MAX));
        let voices = b.load_var(VOICES_FIELD, AccessType::Struct, FirType::Int32);
        let voice = b.load_var(VOICE_ARG, AccessType::FunArgs, FirType::Int32);
        let row = match index {
            Some(i) => b.binop(FirBinOp::Add, base, i, FirType::Int32),
            None => base,
        };
        let scaled = b.binop(FirBinOp::Mul, row, voices, FirType::Int32);
        b.binop(FirBinOp::Add, scaled, voice, FirType::Int32)
    }

    fn placed(&self, name: &str, access: AccessType) -> Option<(String, FirType, usize)> {
        if access != AccessType::Struct {
            return None;
        }
        self.placed.get(name).cloned()
    }

    /// `value` rewritten, cast to the arena's element type when its own type
    /// differs. The producer stores a `Float32` constant into a `FaustFloat`
    /// field with a `StoreVar`, which the FIR verifier does not type; as a
    /// `StoreTable` it does (FIR-T02), so the store's implicit coercion, which
    /// every backend applies, is made explicit.
    fn stored_value(&mut self, value: FirId, elem: &FirType) -> Result<FirId, BatchError> {
        let value = self.rewrite(value)?;
        match self.store.value_type(value) {
            Some(typ) if &typ != elem => Ok(FirBuilder::new(self.store).cast(elem.clone(), value)),
            _ => Ok(value),
        }
    }

    fn element_index(&mut self, field: &str, index: FirId) -> Result<FirId, BatchError> {
        let index = self.rewrite(index)?;
        if self.store.value_type(index) != Some(FirType::Int32) {
            return Err(BatchError::IndexType(field.to_owned()));
        }
        Ok(index)
    }

    fn rewrite(&mut self, id: FirId) -> Result<FirId, BatchError> {
        if let Some(&done) = self.memo.get(&id) {
            return Ok(done);
        }
        let out = match match_fir(self.store, id) {
            FirMatch::LoadVar { name, access, typ } => match self.placed(&name, access) {
                Some((arena, elem, base)) => {
                    if typ != elem {
                        return Err(BatchError::UnsupportedAccess {
                            field: name,
                            what: "a whole-array read",
                        });
                    }
                    let index = self.index(base, None);
                    FirBuilder::new(self.store).load_table(arena, AccessType::Struct, index, typ)
                }
                None => id,
            },
            FirMatch::StoreVar {
                name,
                access,
                value,
            } => match self.placed(&name, access) {
                Some((arena, elem, base)) => {
                    let value = self.stored_value(value, &elem)?;
                    let index = self.index(base, None);
                    FirBuilder::new(self.store).store_table(arena, AccessType::Struct, index, value)
                }
                None => self.rebuild(id)?,
            },
            FirMatch::LoadTable {
                name,
                access,
                index,
                typ,
            } => match self.placed(&name, access) {
                Some((arena, _, base)) => {
                    let i = self.element_index(&name, index)?;
                    let index = self.index(base, Some(i));
                    FirBuilder::new(self.store).load_table(arena, AccessType::Struct, index, typ)
                }
                None => self.rebuild(id)?,
            },
            FirMatch::StoreTable {
                name,
                access,
                index,
                value,
            } => match self.placed(&name, access) {
                Some((arena, elem, base)) => {
                    let i = self.element_index(&name, index)?;
                    let value = self.stored_value(value, &elem)?;
                    let index = self.index(base, Some(i));
                    FirBuilder::new(self.store).store_table(arena, AccessType::Struct, index, value)
                }
                None => self.rebuild(id)?,
            },
            FirMatch::LoadVarAddress { name, access, .. }
                if self.placed(&name, access).is_some() =>
            {
                return Err(BatchError::UnsupportedAccess {
                    field: name,
                    what: "the address",
                });
            }
            FirMatch::TeeVar { name, access, .. } if self.placed(&name, access).is_some() => {
                return Err(BatchError::UnsupportedAccess {
                    field: name,
                    what: "a store used as a value",
                });
            }
            FirMatch::ShiftArrayVar { name, access, .. }
                if self.placed(&name, access).is_some() =>
            {
                return Err(BatchError::UnsupportedAccess {
                    field: name,
                    what: "an array shift",
                });
            }
            _ => self.rebuild(id)?,
        };
        self.memo.insert(id, out);
        Ok(out)
    }

    /// `id` with its children rewritten: the arena node rebuilt with the same
    /// kind, or `id` itself when no child changes.
    fn rebuild(&mut self, id: FirId) -> Result<FirId, BatchError> {
        let Some(children) = self.store.arena.children(id).map(<[FirId]>::to_vec) else {
            return Ok(id);
        };
        if children.is_empty() {
            return Ok(id);
        }
        let mut changed = false;
        let mut rewritten = Vec::with_capacity(children.len());
        for child in children {
            let new = self.rewrite(child)?;
            changed |= new != child;
            rewritten.push(new);
        }
        if !changed {
            return Ok(id);
        }
        let Some(kind) = self.store.arena.kind(id).cloned() else {
            return Ok(id);
        };
        Ok(self.store.arena.intern(kind, &rewritten))
    }
}
