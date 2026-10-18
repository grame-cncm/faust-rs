//! The decoded FIR module every backend starts from, and the stable
//! diagnostics for a malformed one.
//!
//! # Why this exists
//! Each backend used to re-derive the same things from the FIR `Module` root:
//! seven `decode_module`/`ModuleView` pairs, six `find_function_body`, five
//! `block_declares_var`/`block_stores_var`, five `unsupported_node`, and as
//! many spellings of "this section is not a block". The copies had drifted
//! in their messages and in what they kept (see
//! `porting/codegen-factorization-analysis-2026-10-18-en.md`, R1). They live
//! here once. A backend keeps its own error *codes* — a code string is a
//! per-backend contract — and names the three this module raises through
//! [`SkeletonCodes`].
//!
//! # Invariant
//! Nothing here emits target text: these are lookups over the FIR store and
//! the errors for a module the lowering should never produce. Emission stays
//! in each backend.

use fir::{FirId, FirMatch, FirStore, FirType, NamedType, match_fir};

use super::codegen_error::{BackendError, CodegenErrorCode};

/// The error codes a backend raises for a malformed module.
pub(crate) trait SkeletonCodes: CodegenErrorCode {
    /// The root is not a FIR `Module`.
    const ROOT_NOT_MODULE: Self;
    /// A module section (or a `sub_modules` entry) has the wrong shape.
    const INVALID_MODULE_SECTION: Self;
    /// A FIR node this backend does not emit.
    const UNSUPPORTED_NODE: Self;
}

/// The sections of a FIR `Module`, as ids still to be decoded where each one
/// is emitted.
///
/// A short-lived view, not an IR: every id points back into the store.
#[derive(Debug, Clone)]
pub(crate) struct ModuleView {
    pub(crate) name: String,
    pub(crate) dsp_struct: FirId,
    pub(crate) globals: FirId,
    pub(crate) functions: FirId,
    pub(crate) static_decls: FirId,
    pub(crate) sub_modules: FirId,
    pub(crate) num_inputs: usize,
    pub(crate) num_outputs: usize,
}

/// Decodes the FIR module root.
pub(crate) fn decode_module<C: SkeletonCodes>(
    store: &FirStore,
    module: FirId,
) -> Result<ModuleView, BackendError<C>> {
    match match_fir(store, module) {
        FirMatch::Module {
            num_inputs,
            num_outputs,
            name,
            dsp_struct,
            globals,
            functions,
            static_decls,
            sub_modules,
        } => Ok(ModuleView {
            name,
            dsp_struct,
            globals,
            functions,
            static_decls,
            sub_modules,
            num_inputs,
            num_outputs,
        }),
        other => Err(BackendError::new(
            C::ROOT_NOT_MODULE,
            format!(
                "expected FIR module root, got {other:?} at node {}",
                module.as_u32()
            ),
        )
        .at_node(module)),
    }
}

/// The error for a module section that is not a FIR block.
pub(crate) fn invalid_section<C: SkeletonCodes>(
    section: &str,
    node: FirId,
    store: &FirStore,
) -> BackendError<C> {
    BackendError::new(
        C::INVALID_MODULE_SECTION,
        format!(
            "section '{section}' must be a FIR block, got {:?} at node {}",
            match_fir(store, node),
            node.as_u32()
        ),
    )
    .at_node(node)
}

/// The items of a module section, or the error if it is not a block.
pub(crate) fn section_items<C: SkeletonCodes>(
    section: &str,
    node: FirId,
    store: &FirStore,
) -> Result<Vec<FirId>, BackendError<C>> {
    match match_fir(store, node) {
        FirMatch::Block(items) => Ok(items),
        _ => Err(invalid_section(section, node, store)),
    }
}

/// The error for a `sub_modules` entry that is not a `SubModule`.
pub(crate) fn non_sub_module<C: SkeletonCodes>(item: FirId) -> BackendError<C> {
    BackendError::new(
        C::INVALID_MODULE_SECTION,
        format!("sub_modules holds a non-SubModule node {}", item.as_u32()),
    )
    .at_node(item)
}

/// The error for a FIR node the backend does not emit; `kind` says where it
/// was found (`"statement"`, `"value"`, ...).
pub(crate) fn unsupported_node<C: SkeletonCodes>(
    kind: &str,
    node: FirId,
    store: &FirStore,
) -> BackendError<C> {
    BackendError::new(
        C::UNSUPPORTED_NODE,
        format!(
            "unsupported FIR {kind} node {:?} at {}",
            match_fir(store, node),
            node.as_u32()
        ),
    )
    .at_node(node)
}

/// The items of `block`, or none when it is not a block.
///
/// The lenient reading, for callers that treat a missing section as empty;
/// [`section_items`] is the checked one.
pub(crate) fn block_items(store: &FirStore, block: FirId) -> Vec<FirId> {
    match match_fir(store, block) {
        FirMatch::Block(items) => items,
        _ => Vec::new(),
    }
}

/// The body of the first function named `wanted` that has one.
///
/// A body-less declaration (a prototype) is skipped, so a prototype never
/// stands in for a definition.
pub(crate) fn function_body(store: &FirStore, items: &[FirId], wanted: &str) -> Option<FirId> {
    items.iter().find_map(|&item| match match_fir(store, item) {
        FirMatch::DeclareFun {
            name,
            body: Some(body),
            ..
        } if name == wanted => Some(body),
        _ => None,
    })
}

/// [`function_body`] over a `functions` section; none when it is not a block.
pub(crate) fn find_function_body(
    store: &FirStore,
    functions: FirId,
    wanted: &str,
) -> Option<FirId> {
    function_body(store, &block_items(store, functions), wanted)
}

/// Returns `true` when `block` declares a variable named `name`.
pub(crate) fn block_declares_var(store: &FirStore, block: FirId, name: &str) -> bool {
    block_items(store, block).iter().any(|&id| {
        matches!(
            match_fir(store, id),
            FirMatch::DeclareVar { name: var_name, .. } if var_name == name
        )
    })
}

/// Returns `true` when `block` stores to a variable named `name`.
pub(crate) fn block_stores_var(store: &FirStore, block: FirId, name: &str) -> bool {
    block_items(store, block).iter().any(|&id| {
        matches!(
            match_fir(store, id),
            FirMatch::StoreVar { name: var_name, .. } if var_name == name
        )
    })
}

/// Returns `true` when `body` is an empty block.
pub(crate) fn is_empty_block(store: &FirStore, body: FirId) -> bool {
    matches!(match_fir(store, body), FirMatch::Block(items) if items.is_empty())
}

/// Returns `true` for the per-sub-module lifecycle methods
/// (`instanceInit<Sub>`, `fill<Sub>`), which take the sub-module's receiver.
pub(crate) fn is_sub_module_method(name: &str) -> bool {
    name.starts_with("instanceInit") && name != "instanceInit"
        || name.starts_with("fill") && name != "fill"
}

/// One generated-table sub-module, as its fields' ids.
#[derive(Debug, Clone)]
pub(crate) struct SubModuleView {
    pub(crate) name: String,
    pub(crate) elem_type: FirType,
    pub(crate) dsp_struct: FirId,
    pub(crate) static_decls: FirId,
    pub(crate) globals: FirId,
    pub(crate) functions: FirId,
    pub(crate) sub_modules: FirId,
}

/// The sub-modules of a `sub_modules` block, in declaration order; none when
/// it is not a block, an error for an entry that is not a `SubModule`.
pub(crate) fn sub_module_views<C: SkeletonCodes>(
    store: &FirStore,
    sub_modules: FirId,
) -> Result<Vec<SubModuleView>, BackendError<C>> {
    block_items(store, sub_modules)
        .into_iter()
        .map(|item| match match_fir(store, item) {
            FirMatch::SubModule {
                name,
                elem_type,
                dsp_struct,
                static_decls,
                globals,
                functions,
                sub_modules,
            } => Ok(SubModuleView {
                name,
                elem_type,
                dsp_struct,
                static_decls,
                globals,
                functions,
                sub_modules,
            }),
            _ => Err(non_sub_module(item)),
        })
        .collect()
}

/// The error message for sub-modules left in `module` after flattening, if
/// any.
///
/// Flattening removes them; a survivor is an internal error and must not
/// reach the output, because a table declared and never filled reads as zeros.
pub(crate) fn surviving_sub_modules(store: &FirStore, module: FirId) -> Option<String> {
    let FirMatch::Module { sub_modules, .. } = match_fir(store, module) else {
        return None;
    };
    let names = super::sub_module_names(store, sub_modules);
    (!names.is_empty()).then(|| {
        format!(
            "sub-modules survived flattening ({}); this is an internal error",
            names.join(", ")
        )
    })
}

/// Prepares a module for a backend with no nested containers: inlines its
/// generated-table sub-modules under `policy`, optionally schedules `compute`
/// for a JIT (`fir::schedule`), and checks that no sub-module survived.
///
/// Returns the rewritten store and root, or `None` when nothing changed and
/// the caller keeps its own. The error is the message; each backend wraps it
/// in its own code.
pub(crate) fn prepare_flat_module(
    store: &FirStore,
    module: FirId,
    policy: fir::subcontainer::SubModuleStatePolicy,
    schedule_compute: bool,
) -> Result<Option<(FirStore, FirId)>, String> {
    let flattened = if fir::subcontainer::has_sub_modules(store, module) {
        Some(
            fir::subcontainer::flatten_sub_modules_owned(store, module, policy)
                .map_err(|err| format!("flattening generated tables failed: {err}"))?,
        )
    } else {
        None
    };
    let (current, root) = flattened.as_ref().map_or((store, module), |(s, m)| (s, *m));
    let scheduled = if schedule_compute {
        fir::schedule::schedule_compute(current, root)
    } else {
        None
    };
    let prepared = scheduled.or(flattened);
    let (current, root) = prepared.as_ref().map_or((store, module), |(s, m)| (s, *m));
    match surviving_sub_modules(current, root) {
        Some(message) => Err(message),
        None => Ok(prepared),
    }
}

/// One function definition of the `functions` section.
#[derive(Debug, Clone)]
pub(crate) struct FunctionDecl {
    pub(crate) name: String,
    pub(crate) typ: FirType,
    pub(crate) named_args: Vec<NamedType>,
    /// Always `Some` from [`collect_defined_functions`]; kept optional so a
    /// backend can build a declaration-only view.
    pub(crate) body: Option<FirId>,
}

/// The function *definitions* of a `functions` section, in declaration order.
///
/// Prototype-only declarations are skipped: they are not executable bodies
/// and must not suppress the lifecycle stub a backend synthesizes for a
/// function the module does not define.
pub(crate) fn collect_defined_functions<C: SkeletonCodes>(
    store: &FirStore,
    functions: FirId,
) -> Result<Vec<FunctionDecl>, BackendError<C>> {
    let items = section_items("functions", functions, store)?;
    Ok(items
        .into_iter()
        .filter_map(|item| match match_fir(store, item) {
            FirMatch::DeclareFun {
                name,
                typ,
                args,
                body: Some(body),
                ..
            } => Some(FunctionDecl {
                name,
                typ,
                named_args: args,
                body: Some(body),
            }),
            _ => None,
        })
        .collect())
}

/// The names of every function a `functions` section declares, prototypes
/// included, in declaration order.
pub(crate) fn collect_declared_function_names<C: SkeletonCodes>(
    store: &FirStore,
    functions: FirId,
) -> Result<Vec<String>, BackendError<C>> {
    let items = section_items("functions", functions, store)?;
    Ok(items
        .into_iter()
        .filter_map(|item| match match_fir(store, item) {
            FirMatch::DeclareFun { name, .. } => Some(name),
            _ => None,
        })
        .collect())
}

/// The methods of the Faust `dsp` class API (`faust/dsp/dsp.h`).
const DSP_API_METHODS: [&str; 14] = [
    "metadata",
    "getNumInputs",
    "getNumOutputs",
    "getSampleRate",
    "classInit",
    "instanceConstants",
    "instanceResetUserInterface",
    "instanceClear",
    "instanceInit",
    "init",
    "buildUserInterface",
    "control",
    "frame",
    "compute",
];

/// Returns `true` for a method of the Faust `dsp` class API.
pub(crate) fn is_dsp_api_method(name: &str) -> bool {
    DSP_API_METHODS.contains(&name)
}

/// Returns `true` for a lifecycle function the FIR lowers with an explicit
/// `dsp` receiver: every [`super::is_lifecycle_function`] but `staticInit`,
/// which is class-level. A target with an implicit receiver (`this`) drops
/// that first argument.
pub(crate) fn is_receiver_lifecycle_method(name: &str) -> bool {
    name != "staticInit" && super::is_lifecycle_function(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::cpp::{CodegenError, CodegenErrorCode};
    use fir::{AccessType, FirBuilder, FirType};

    #[test]
    fn decode_rejects_a_non_module_root_with_its_node() {
        let mut store = FirStore::new();
        let root = FirBuilder::new(&mut store).int32(7);
        let err: CodegenError = decode_module(&store, root).unwrap_err();
        assert_eq!(err.code(), CodegenErrorCode::RootNotModule);
        assert_eq!(err.fir_node(), Some(root));
        assert!(
            err.message()
                .starts_with("expected FIR module root, got Int32")
        );
    }

    #[test]
    fn section_checks_name_the_section_and_node() {
        let mut store = FirStore::new();
        let not_a_block = FirBuilder::new(&mut store).int32(1);
        let err: CodegenError = section_items("functions", not_a_block, &store).unwrap_err();
        assert_eq!(err.code(), CodegenErrorCode::InvalidModuleSection);
        assert_eq!(err.fir_node(), Some(not_a_block));
        assert!(
            err.message()
                .starts_with("section 'functions' must be a FIR block, got Int32")
        );
    }

    #[test]
    fn a_prototype_never_stands_in_for_a_definition() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let fun_type = FirType::Fun {
            args: vec![],
            ret: Box::new(FirType::Void),
        };
        let prototype = b.declare_fun("instanceClear", fun_type.clone(), &[], None, false);
        let body = b.block(&[]);
        let definition = b.declare_fun("instanceClear", fun_type, &[], Some(body), false);
        assert_eq!(function_body(&store, &[prototype], "instanceClear"), None);
        assert_eq!(
            function_body(&store, &[prototype, definition], "instanceClear"),
            Some(body)
        );
    }

    #[test]
    fn receiver_methods_are_the_lifecycle_functions_but_static_init() {
        for name in [
            "metadata",
            "instanceConstants",
            "instanceResetUserInterface",
            "instanceClear",
            "buildUserInterface",
            "compute",
            "control",
            "frame",
        ] {
            assert!(is_receiver_lifecycle_method(name), "{name}");
            assert!(is_dsp_api_method(name), "{name}");
        }
        assert!(!is_receiver_lifecycle_method("staticInit"));
        assert!(!is_receiver_lifecycle_method("init"));
        assert!(is_dsp_api_method("init"));
        assert!(!is_dsp_api_method("staticInit"));
    }

    #[test]
    fn a_flat_module_is_left_alone_unless_scheduled() {
        let (store, module) = crate::fixtures::build_passthrough_test_module();
        let prepared = prepare_flat_module(
            &store,
            module,
            fir::subcontainer::SubModuleStatePolicy::MergedStructFields,
            false,
        )
        .expect("a module without sub-modules prepares");
        assert!(prepared.is_none());
        assert_eq!(surviving_sub_modules(&store, module), None);
    }

    #[test]
    fn variable_lookups_see_declarations_and_stores() {
        let mut store = FirStore::new();
        let mut b = FirBuilder::new(&mut store);
        let decl = b.declare_var("fSampleRate", FirType::Int32, AccessType::Struct, None);
        let value = b.int32(0);
        let assign = b.store_var("fSampleRate", AccessType::Struct, value);
        let block = b.block(&[decl, assign]);
        assert!(block_declares_var(&store, block, "fSampleRate"));
        assert!(block_stores_var(&store, block, "fSampleRate"));
        assert!(!block_declares_var(&store, block, "fOther"));
        assert!(!is_empty_block(&store, block));
    }
}
