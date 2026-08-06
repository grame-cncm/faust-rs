//! Sub-module flattening for backends that cannot express a nested container.
//!
//! Port of the C++ `inlineSubcontainersFunCalls` + `DspRenamer` +
//! `mergeSubContainers` trio (`compiler/generator/code_container.cpp`), which
//! upstream applies for `interp`, `wasm`, `jsfx`, `codebox` and `julia` while
//! the object-oriented textual backends emit the nested container directly.
//!
//! # What it does
//!
//! A table generator is produced as a [`FirMatch::SubModule`] with its own
//! state and two entry points, and the enclosing program calls them as
//!
//! ```text
//! sig0 = new mydspSIG0
//! instanceInitmydspSIG0(sig0, sample_rate)
//! fillmydspSIG0(sig0, 65536, ftbl0mydspSIG0)
//! ```
//!
//! This pass replaces those three statements with the sub-module's own bodies,
//! rewriting `count` and `table` to what the call site passed and moving the
//! sub-module's state, static tables and prototypes into the enclosing program.
//!
//! # Why the state policy matters
//!
//! In C++ the sub-container is an object, so its state lives wherever that
//! object does. Flattening has to put it somewhere concrete, and the right
//! answer differs per backend:
//!
//! - [`SubModuleStatePolicy::StackLocals`] demotes it to locals of the
//!   initialization function. This is what a target with a `static classInit`
//!   needs, since a static method has no instance to hold fields.
//! - [`SubModuleStatePolicy::MergedStructFields`] keeps it in the DSP struct
//!   under a prefixed name, which is what upstream `mergeSubContainers` does
//!   and what heap-based backends (`interp`, `wasm`) require, their
//!   `classInit` being instance-scoped.
//!
//! Either way the names are prefixed with the sub-module's, because a
//! sub-module is lowered with its own fresh name counters and its `iRec0` is
//! unrelated to the enclosing program's `iRec0`.

use std::collections::{BTreeSet, HashMap};

use crate::inliner::{FirHygienicCloneState, flatten_clone_body};
use crate::{AccessType, FirBuilder, FirId, FirMatch, FirStore, FirType, match_fir};

/// Where a flattened sub-module's own state is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubModuleStatePolicy {
    /// Demote sub-module state to locals of the enclosing initialization
    /// function. Required when `classInit` is emitted as a static method.
    StackLocals,
    /// Merge sub-module state into the DSP struct under a prefixed name.
    /// C++ parity: `mergeSubContainers`.
    MergedStructFields,
}

/// Why flattening could not be performed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlattenError {
    /// The root node is not a `Module`.
    RootNotModule(FirId),
    /// A `sub_modules` entry is not a `SubModule`.
    NotASubModule(FirId),
    /// A sub-module is missing one of its two entry points.
    MissingEntryPoint {
        /// Sub-module name.
        sub_module: String,
        /// Function that could not be found.
        function: String,
    },
    /// A `fill` call named a sub-module the module does not declare.
    UnknownSubModule(String),
    /// Hygienic cloning failed while inlining a body.
    Clone(String),
}

impl std::fmt::Display for FlattenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootNotModule(id) => write!(f, "root node {} is not a Module", id.as_u32()),
            Self::NotASubModule(id) => {
                write!(f, "sub_modules entry {} is not a SubModule", id.as_u32())
            }
            Self::MissingEntryPoint {
                sub_module,
                function,
            } => write!(f, "sub-module '{sub_module}' has no '{function}'"),
            Self::UnknownSubModule(name) => write!(f, "call to unknown sub-module '{name}'"),
            Self::Clone(message) => write!(f, "clone failed while flattening: {message}"),
        }
    }
}

impl std::error::Error for FlattenError {}

/// One sub-module's parts, indexed by name.
///
/// `static_decls` and `globals` are hoisted at collection time rather than
/// stored here, and `elem_type` is only needed by backends that emit the
/// nested form, so neither is carried past indexing.
struct SubModuleView {
    dsp_struct: FirId,
    init_body: FirId,
    fill_body: FirId,
}

/// Inlines every sub-module of `module` into the program that calls it.
///
/// Returns a new module root with an empty `sub_modules` block, whose
/// `staticInit`/`instanceConstants` bodies contain the generator code inline.
/// A module without sub-modules is returned unchanged.
///
/// # Errors
/// Returns [`FlattenError`] when the root is not a module, a sub-module is
/// malformed, or a body could not be cloned.
pub fn flatten_sub_modules(
    store: &mut FirStore,
    module: FirId,
    policy: SubModuleStatePolicy,
) -> Result<FirId, FlattenError> {
    let FirMatch::Module {
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        sub_modules,
    } = match_fir(store, module)
    else {
        return Err(FlattenError::RootNotModule(module));
    };

    let FirMatch::Block(sub_ids) = match_fir(store, sub_modules) else {
        return Err(FlattenError::NotASubModule(sub_modules));
    };
    if sub_ids.is_empty() {
        return Ok(module);
    }

    // Deepest-first: a sub-module that owns nested generators is flattened
    // into itself before the enclosing program inlines it, so the nested fill
    // has already become straight-line code by the time we splice it.
    let mut views: HashMap<String, SubModuleView> = HashMap::new();
    let mut hoisted_struct = Vec::new();
    let mut hoisted_static = Vec::new();
    let mut hoisted_globals = Vec::new();
    collect_sub_modules(
        store,
        &sub_ids,
        policy,
        &mut views,
        &mut hoisted_struct,
        &mut hoisted_static,
        &mut hoisted_globals,
    )?;

    let functions = rewrite_functions(store, functions, &views, policy)?;

    let dsp_struct = append_block(store, dsp_struct, &hoisted_struct);
    let static_decls = append_block(store, static_decls, &hoisted_static);
    let globals = append_block(store, globals, &hoisted_globals);

    let mut b = FirBuilder::new(store);
    Ok(b.module(
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        &[],
    ))
}

/// Flattens into a freshly owned store.
///
/// Backends receive `&FirStore` and cannot mutate it, so this imports the
/// module into a new store first. The returned store owns every id in the
/// returned root.
///
/// # Errors
/// Returns [`FlattenError`] for the same reasons as [`flatten_sub_modules`].
pub fn flatten_sub_modules_owned(
    src: &FirStore,
    module: FirId,
    policy: SubModuleStatePolicy,
) -> Result<(FirStore, FirId), FlattenError> {
    let mut dst = FirStore::new();
    let root = dst.import_from(src, module);
    let flattened = flatten_sub_modules(&mut dst, root, policy)?;
    Ok((dst, flattened))
}

/// Moves runtime-filled tables from file scope into the DSP struct.
///
/// Some targets have no shared static storage: Julia's `classInit!` takes the
/// DSP and the reference emits `dsp.ftbl0mydspSIG0`, and Cmajor keeps tables as
/// processor fields for the same reason
/// (`porting/cmajor-backend-port-and-test-plan-2026-08-04-en.md` §4.5).
///
/// Rewriting the declaration's access class rather than special-casing the
/// emitters means every reference to the table renders through the backend's
/// existing struct-field path, with no per-site knowledge of which tables moved.
/// Folded tables (`DeclareTable`, immutable, with their data inline) are left
/// at file scope: they need no instance and duplicating them per instance would
/// be a regression.
///
/// # Errors
/// Returns [`FlattenError`] when the root is not a module.
pub fn promote_static_tables_to_struct(
    store: &mut FirStore,
    module: FirId,
) -> Result<FirId, FlattenError> {
    let FirMatch::Module {
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        sub_modules,
    } = match_fir(store, module)
    else {
        return Err(FlattenError::RootNotModule(module));
    };
    let FirMatch::Block(statics) = match_fir(store, static_decls) else {
        return Err(FlattenError::RootNotModule(module));
    };

    let mut promoted = Vec::new();
    let mut kept = Vec::new();
    for item in statics {
        match match_fir(store, item) {
            FirMatch::DeclareVar {
                name,
                typ: typ @ FirType::Array(..),
                init: None,
                ..
            } => {
                let mut b = FirBuilder::new(store);
                promoted.push(b.declare_var(name, typ, AccessType::Struct, None));
            }
            _ => kept.push(item),
        }
    }
    if promoted.is_empty() {
        return Ok(module);
    }

    let dsp_struct = append_block(store, dsp_struct, &promoted);
    let static_decls = {
        let mut b = FirBuilder::new(store);
        b.block(&kept)
    };
    let functions =
        rewrite_static_table_access(store, functions, &promoted_names(store, &promoted));

    // Sub-modules are carried across untouched. This pass normally runs after
    // flattening, where none are left, but discarding them here would silently
    // drop a generator if it were ever run on a module that still had them.
    let carried = match match_fir(store, sub_modules) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    };
    let mut b = FirBuilder::new(store);
    Ok(b.module(
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        &carried,
    ))
}

/// Rewrites every reference to a promoted table from `Static` to `Struct`.
fn rewrite_static_table_access(
    store: &mut FirStore,
    functions: FirId,
    promoted: &BTreeSet<String>,
) -> FirId {
    let subst: HashMap<String, (String, AccessType)> = promoted
        .iter()
        .map(|name| (name.clone(), (name.clone(), AccessType::Struct)))
        .collect();
    let mut state = FirHygienicCloneState::default();
    crate::inliner::flatten_clone_body_with_static_subst(store, functions, &mut state, &subst)
        .unwrap_or(functions)
}

/// Names of the promoted declarations.
fn promoted_names(store: &FirStore, promoted: &[FirId]) -> BTreeSet<String> {
    promoted
        .iter()
        .filter_map(|id| match match_fir(store, *id) {
            FirMatch::DeclareVar { name, .. } => Some(name),
            _ => None,
        })
        .collect()
}

/// Qualifies every `kStruct` reference in `body` with an explicit receiver.
///
/// Some targets address struct members only through a named receiver: Cmajor
/// functions are written `void f (Sub& this, …)` and reject a bare `fConst0`
/// with "Cannot find symbol". Rewriting the references — rather than teaching
/// the emitter where it currently is — keeps the knowledge in one place and
/// costs the emitter nothing.
///
/// Only the body is rewritten; the struct's own field *declarations* keep their
/// plain names, which is what makes this safe to apply to a sub-module's
/// functions without touching its type.
///
/// # Errors
/// Returns [`FlattenError`] when the body contains a node the clone engine does
/// not handle.
pub fn qualify_struct_access(
    store: &mut FirStore,
    body: FirId,
    fields: &BTreeSet<String>,
    receiver: &str,
) -> Result<FirId, FlattenError> {
    if fields.is_empty() {
        return Ok(body);
    }
    let subst: HashMap<String, (String, AccessType)> = fields
        .iter()
        .map(|name| {
            (
                name.clone(),
                (format!("{receiver}.{name}"), AccessType::Struct),
            )
        })
        .collect();
    let mut state = FirHygienicCloneState::default();
    crate::inliner::qualify_clone_body(store, body, &mut state, &subst)
        .map_err(|err| FlattenError::Clone(format!("{err:?}")))
}

/// Qualifies every sub-module's function bodies with an explicit receiver.
///
/// A whole-module normalization for targets whose functions address struct
/// members only through a named receiver, applied once before emission so the
/// emitter itself needs no notion of where it is. Nested sub-modules are
/// rewritten too.
///
/// # Errors
/// Returns [`FlattenError`] when the root is not a module or a body could not
/// be rewritten.
pub fn qualify_sub_module_bodies(
    store: &mut FirStore,
    module: FirId,
    receiver: &str,
) -> Result<FirId, FlattenError> {
    let FirMatch::Module {
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        sub_modules,
    } = match_fir(store, module)
    else {
        return Err(FlattenError::RootNotModule(module));
    };
    let FirMatch::Block(subs) = match_fir(store, sub_modules) else {
        return Ok(module);
    };
    if subs.is_empty() {
        return Ok(module);
    }

    let mut rewritten = Vec::with_capacity(subs.len());
    for sub in subs {
        rewritten.push(qualify_one_sub_module(store, sub, receiver)?);
    }

    let mut b = FirBuilder::new(store);
    Ok(b.module(
        num_inputs,
        num_outputs,
        name,
        dsp_struct,
        globals,
        functions,
        static_decls,
        &rewritten,
    ))
}

/// Rewrites one sub-module's function bodies, recursing into nested ones.
fn qualify_one_sub_module(
    store: &mut FirStore,
    sub: FirId,
    receiver: &str,
) -> Result<FirId, FlattenError> {
    let FirMatch::SubModule {
        name,
        elem_type,
        dsp_struct,
        static_decls,
        globals,
        functions,
        sub_modules,
    } = match_fir(store, sub)
    else {
        return Err(FlattenError::NotASubModule(sub));
    };

    let fields = struct_field_names(store, dsp_struct);
    let FirMatch::Block(items) = match_fir(store, functions) else {
        return Ok(sub);
    };
    let mut new_functions = Vec::with_capacity(items.len());
    for item in items {
        let FirMatch::DeclareFun {
            name: fun_name,
            typ,
            args,
            body: Some(body),
            is_inline,
        } = match_fir(store, item)
        else {
            new_functions.push(item);
            continue;
        };
        let body = qualify_struct_access(store, body, &fields, receiver)?;
        let mut b = FirBuilder::new(store);
        new_functions.push(b.declare_fun(fun_name, typ, &args, Some(body), is_inline));
    }

    let mut nested = Vec::new();
    if let FirMatch::Block(inner) = match_fir(store, sub_modules) {
        for one in inner {
            nested.push(qualify_one_sub_module(store, one, receiver)?);
        }
    }

    let mut b = FirBuilder::new(store);
    let functions = b.block(&new_functions);
    Ok(b.sub_module(
        name,
        elem_type,
        dsp_struct,
        static_decls,
        globals,
        functions,
        &nested,
    ))
}

/// Names of the fields a struct block declares.
#[must_use]
pub fn struct_field_names(store: &FirStore, dsp_struct: FirId) -> BTreeSet<String> {
    let FirMatch::Block(items) = match_fir(store, dsp_struct) else {
        return BTreeSet::new();
    };
    items
        .into_iter()
        .filter_map(|item| match match_fir(store, item) {
            FirMatch::DeclareVar { name, .. } | FirMatch::DeclareTable { name, .. } => Some(name),
            _ => None,
        })
        .collect()
}

/// Returns `true` when a module declares at least one generated-table
/// sub-module.
#[must_use]
pub fn has_sub_modules(store: &FirStore, module: FirId) -> bool {
    let FirMatch::Module { sub_modules, .. } = match_fir(store, module) else {
        return false;
    };
    matches!(match_fir(store, sub_modules), FirMatch::Block(items) if !items.is_empty())
}

/// Indexes sub-modules by name and collects the declarations to hoist.
fn collect_sub_modules(
    store: &mut FirStore,
    sub_ids: &[FirId],
    policy: SubModuleStatePolicy,
    views: &mut HashMap<String, SubModuleView>,
    hoisted_struct: &mut Vec<FirId>,
    hoisted_static: &mut Vec<FirId>,
    hoisted_globals: &mut Vec<FirId>,
) -> Result<(), FlattenError> {
    for id in sub_ids {
        let FirMatch::SubModule {
            name,
            dsp_struct,
            static_decls,
            globals,
            functions,
            sub_modules,
            ..
        } = match_fir(store, *id)
        else {
            return Err(FlattenError::NotASubModule(*id));
        };

        // Nested generators first.
        if let FirMatch::Block(nested) = match_fir(store, sub_modules)
            && !nested.is_empty()
        {
            collect_sub_modules(
                store,
                &nested,
                policy,
                views,
                hoisted_struct,
                hoisted_static,
                hoisted_globals,
            )?;
        }

        let init_body =
            entry_body(store, functions, &format!("instanceInit{name}")).ok_or_else(|| {
                FlattenError::MissingEntryPoint {
                    sub_module: name.clone(),
                    function: format!("instanceInit{name}"),
                }
            })?;
        let fill_body = entry_body(store, functions, &format!("fill{name}")).ok_or_else(|| {
            FlattenError::MissingEntryPoint {
                sub_module: name.clone(),
                function: format!("fill{name}"),
            }
        })?;

        // Under `MergedStructFields` the state becomes prefixed DSP fields;
        // under `StackLocals` it is declared inside the initialization
        // function instead, so nothing is hoisted here.
        if policy == SubModuleStatePolicy::MergedStructFields
            && let FirMatch::Block(fields) = match_fir(store, dsp_struct)
        {
            for field in fields {
                if let Some(renamed) = rename_declaration(store, field, &name, AccessType::Struct) {
                    hoisted_struct.push(renamed);
                }
            }
        }
        if let FirMatch::Block(items) = match_fir(store, static_decls) {
            hoisted_static.extend(items);
        }
        if let FirMatch::Block(items) = match_fir(store, globals) {
            hoisted_globals.extend(items);
        }

        views.insert(
            name,
            SubModuleView {
                dsp_struct,
                init_body,
                fill_body,
            },
        );
    }
    Ok(())
}

/// Returns the body of the named function inside a `functions` block.
fn entry_body(store: &FirStore, functions: FirId, wanted: &str) -> Option<FirId> {
    let FirMatch::Block(items) = match_fir(store, functions) else {
        return None;
    };
    items
        .into_iter()
        .find_map(|item| match match_fir(store, item) {
            FirMatch::DeclareFun { name, body, .. } if name == wanted => body,
            _ => None,
        })
}

/// Re-declares one sub-module state field under its prefixed name.
fn rename_declaration(
    store: &mut FirStore,
    decl: FirId,
    sub_module: &str,
    access: AccessType,
) -> Option<FirId> {
    match match_fir(store, decl) {
        FirMatch::DeclareVar {
            name, typ, init, ..
        } => {
            let mut b = FirBuilder::new(store);
            Some(b.declare_var(prefixed(sub_module, &name), typ, access, init))
        }
        FirMatch::DeclareTable {
            name,
            elem_type,
            values,
            ..
        } => {
            let mut b = FirBuilder::new(store);
            Some(b.declare_table(prefixed(sub_module, &name), access, elem_type, &values))
        }
        _ => None,
    }
}

/// The flattened name of one sub-module-local symbol.
fn prefixed(sub_module: &str, name: &str) -> String {
    format!("{sub_module}_{name}")
}

/// Appends `extra` to a block, preserving order.
fn append_block(store: &mut FirStore, block: FirId, extra: &[FirId]) -> FirId {
    if extra.is_empty() {
        return block;
    }
    let mut items = match match_fir(store, block) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    };
    items.extend_from_slice(extra);
    let mut b = FirBuilder::new(store);
    b.block(&items)
}

/// Rewrites every lifecycle body that calls a sub-module.
fn rewrite_functions(
    store: &mut FirStore,
    functions: FirId,
    views: &HashMap<String, SubModuleView>,
    policy: SubModuleStatePolicy,
) -> Result<FirId, FlattenError> {
    let FirMatch::Block(items) = match_fir(store, functions) else {
        return Ok(functions);
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let FirMatch::DeclareFun {
            name,
            typ,
            args,
            body: Some(body),
            is_inline,
        } = match_fir(store, item)
        else {
            out.push(item);
            continue;
        };
        let FirMatch::Block(statements) = match_fir(store, body) else {
            out.push(item);
            continue;
        };
        let rewritten = rewrite_statements(store, &statements, views, policy)?;
        if rewritten == statements {
            out.push(item);
            continue;
        }
        let new_body = {
            let mut b = FirBuilder::new(store);
            b.block(&rewritten)
        };
        let mut b = FirBuilder::new(store);
        out.push(b.declare_fun(name, typ, &args, Some(new_body), is_inline));
    }
    let mut b = FirBuilder::new(store);
    Ok(b.block(&out))
}

/// Splices sub-module bodies in place of their allocate/init/fill calls.
fn rewrite_statements(
    store: &mut FirStore,
    statements: &[FirId],
    views: &HashMap<String, SubModuleView>,
    policy: SubModuleStatePolicy,
) -> Result<Vec<FirId>, FlattenError> {
    let mut out = Vec::with_capacity(statements.len());
    for stmt in statements {
        // `sigN = new mydspSIG0` disappears: after flattening there is no
        // object, only the code its methods contained. The allocation is
        // dropped in whichever shape it reaches us — the hygienic clone engine
        // may hoist the `NewDsp` out of its declaration into a statement of
        // its own, so matching only the declaration form would silently leave
        // a dangling allocation behind.
        if is_allocation(store, *stmt) {
            continue;
        }

        let Some((callee, args)) = dropped_call(store, *stmt) else {
            out.push(*stmt);
            continue;
        };
        // The sub-module is resolved from the callee name, not from the
        // receiver: the name is stable under cloning, the receiver variable is
        // not.
        let Some(sub_name) = callee_sub_module(&callee, views) else {
            out.push(*stmt);
            continue;
        };
        let view = views
            .get(&sub_name)
            .ok_or_else(|| FlattenError::UnknownSubModule(sub_name.clone()))?;

        if callee == format!("instanceInit{sub_name}") {
            let mut prelude = Vec::new();
            if policy == SubModuleStatePolicy::StackLocals
                && let FirMatch::Block(fields) = match_fir(store, view.dsp_struct)
            {
                for field in fields {
                    if let Some(local) =
                        rename_declaration(store, field, &sub_name, AccessType::Stack)
                    {
                        prelude.push(local);
                    }
                }
            }
            out.extend(prelude);
            // The spliced body may itself contain an allocate/init/fill triple
            // for a nested generator, so it goes through the same rewrite. The
            // views map is populated deepest-first, so the nested entry is
            // already there.
            let inlined = inline_body(
                store,
                view.init_body,
                &sub_name,
                view.dsp_struct,
                policy,
                &HashMap::new(),
            )?;
            out.extend(rewrite_statements(store, &inlined, views, policy)?);
            continue;
        }

        if callee == format!("fill{sub_name}") {
            // `fill(obj, count, table)`: bind `count` to a local holding the
            // constant the caller passed, and `table` to the caller's own
            // table with its own storage class.
            let (count_value, table_ref) = (args.get(1).copied(), args.get(2).copied());
            let mut subst: HashMap<String, (String, AccessType)> = HashMap::new();
            if let Some(count_value) = count_value {
                let count_name = prefixed(&sub_name, "count");
                let decl = {
                    let mut b = FirBuilder::new(store);
                    b.declare_var(
                        count_name.clone(),
                        FirType::Int32,
                        AccessType::Stack,
                        Some(count_value),
                    )
                };
                out.push(decl);
                subst.insert("count".to_string(), (count_name, AccessType::Stack));
            }
            if let Some(table_ref) = table_ref
                && let FirMatch::LoadVar { name, access, .. } = match_fir(store, table_ref)
            {
                subst.insert("table".to_string(), (name, access));
            }
            let inlined = inline_body(
                store,
                view.fill_body,
                &sub_name,
                view.dsp_struct,
                policy,
                &subst,
            )?;
            out.extend(rewrite_statements(store, &inlined, views, policy)?);
            continue;
        }

        out.push(*stmt);
    }
    Ok(out)
}

/// Decodes `Drop(FunCall(name, args))`, the shape the producer emits for a
/// void call.
fn dropped_call(store: &FirStore, stmt: FirId) -> Option<(String, Vec<FirId>)> {
    let FirMatch::Drop(value) = match_fir(store, stmt) else {
        return None;
    };
    match match_fir(store, value) {
        FirMatch::FunCall { name, args, .. } => Some((name, args)),
        _ => None,
    }
}

/// Returns `true` when a statement is (or declares) a sub-container
/// allocation.
fn is_allocation(store: &FirStore, stmt: FirId) -> bool {
    match match_fir(store, stmt) {
        FirMatch::NewDsp { .. } => true,
        FirMatch::DeclareVar {
            init: Some(init), ..
        } => {
            matches!(match_fir(store, init), FirMatch::NewDsp { .. })
        }
        FirMatch::Drop(value) => matches!(match_fir(store, value), FirMatch::NewDsp { .. }),
        _ => false,
    }
}

/// Resolves which sub-module a call belongs to, from the callee name.
fn callee_sub_module(callee: &str, views: &HashMap<String, SubModuleView>) -> Option<String> {
    for prefix in ["instanceInit", "fill"] {
        if let Some(rest) = callee.strip_prefix(prefix)
            && views.contains_key(rest)
        {
            return Some(rest.to_string());
        }
    }
    None
}

/// Clones one sub-module body into the caller, renaming its state and
/// substituting its parameters.
fn inline_body(
    store: &mut FirStore,
    body: FirId,
    sub_module: &str,
    dsp_struct: FirId,
    policy: SubModuleStatePolicy,
    fun_arg_subst: &HashMap<String, (String, AccessType)>,
) -> Result<Vec<FirId>, FlattenError> {
    let target_access = match policy {
        SubModuleStatePolicy::StackLocals => AccessType::Stack,
        SubModuleStatePolicy::MergedStructFields => AccessType::Struct,
    };
    let mut struct_subst: HashMap<String, (String, AccessType)> = HashMap::new();
    if let FirMatch::Block(fields) = match_fir(store, dsp_struct) {
        for field in fields {
            let field_name = match match_fir(store, field) {
                FirMatch::DeclareVar { name, .. } | FirMatch::DeclareTable { name, .. } => name,
                _ => continue,
            };
            struct_subst.insert(
                field_name.clone(),
                (prefixed(sub_module, &field_name), target_access),
            );
        }
    }

    let mut state = FirHygienicCloneState::default();
    let cloned = flatten_clone_body(store, body, &mut state, fun_arg_subst, &struct_subst)
        .map_err(|err| FlattenError::Clone(format!("{err:?}")))?;
    match match_fir(store, cloned) {
        FirMatch::Block(items) => Ok(items),
        _ => Ok(vec![cloned]),
    }
}

/// Structural report on one flattened module.
///
/// Produced by [`verify_flattened`], which is deliberately independent of
/// [`flatten_sub_modules`]: it re-derives what a correct flattening must look
/// like from the module alone, so a bug in the pass shows up as a failed
/// check rather than as agreement between a producer and its own reasoning.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlattenReport {
    /// Structural violations found, in discovery order.
    pub problems: Vec<String>,
}

impl FlattenReport {
    /// Whether the flattened module is structurally sound.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Checks that a module has been flattened correctly.
///
/// Verifies three things a broken pass would get wrong:
///
/// 1. no `sub_modules` remain, and no `SubModule` node is reachable;
/// 2. no call to a sub-module entry point survives — an `instanceInit…` or
///    `fill…` call after flattening would reference a function that no longer
///    exists, which the C backends would only discover at compile time;
/// 3. no `NewDsp` allocation survives, since the object it allocated is gone;
/// 4. every reachable node still decodes. A dangling id decodes as `Unknown`,
///    which most consumers skip rather than reject, so it travels silently
///    until a backend refuses it — which is how the clone-into-emptied-store
///    bug in `flatten_clone_body` was found, well after these checks first
///    passed against its output.
///
/// A module that was never flattened (no sub-modules to begin with) is clean
/// by construction.
#[must_use]
pub fn verify_flattened(store: &FirStore, module: FirId) -> FlattenReport {
    let mut report = FlattenReport::default();
    let FirMatch::Module {
        functions,
        sub_modules,
        ..
    } = match_fir(store, module)
    else {
        report.problems.push("root is not a Module".to_string());
        return report;
    };

    if let FirMatch::Block(items) = match_fir(store, sub_modules)
        && !items.is_empty()
    {
        report.problems.push(format!(
            "{} sub-module(s) still declared after flattening",
            items.len()
        ));
    }

    let declared = declared_function_names(store, functions);
    let mut stack = vec![functions];
    while let Some(id) = stack.pop() {
        match match_fir(store, id) {
            FirMatch::Unknown => {
                report.problems.push(format!(
                    "node {} does not decode after flattening; the id is dangling",
                    id.as_u32()
                ));
            }
            FirMatch::SubModule { name, .. } => {
                report
                    .problems
                    .push(format!("SubModule '{name}' still reachable"));
            }
            FirMatch::NewDsp { name, .. } => {
                report
                    .problems
                    .push(format!("allocation of '{name}' survived flattening"));
            }
            FirMatch::FunCall { ref name, .. }
                if (name.starts_with("fill") || name.starts_with("instanceInit"))
                    && !declared.contains(name) =>
            {
                report.problems.push(format!(
                    "call to '{name}' has no declaration after flattening"
                ));
            }
            _ => {}
        }
        stack.extend(crate::child_ids(&match_fir(store, id)));
    }
    report
}

/// Collects the names of functions a module declares.
fn declared_function_names(store: &FirStore, functions: FirId) -> Vec<String> {
    let FirMatch::Block(items) = match_fir(store, functions) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| match match_fir(store, item) {
            FirMatch::DeclareFun { name, .. } => Some(name),
            _ => None,
        })
        .collect()
}
