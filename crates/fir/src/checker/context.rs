//! `context` rules of the FIR checker.
//!
//! Diagnostic emission, scope entry and exit, and name resolution: the machinery every rule uses.
//!
//! Split out of `checker.rs` on 2026-08-18, where all 67 methods sat in one
//! 2674-line `impl`. Bodies are moved verbatim; the only edit is visibility,
//! private to `pub(super)`, so sibling rule modules can still call across.

use super::*;

impl<'s> VerifyCtx<'s> {
    /// Creates a new verifier context rooted at `module_id`.
    ///
    /// For full-module verification `module_id` is the FIR `Module` node; for
    /// single-function verification it may temporarily be a `DeclareFun`.
    pub(super) fn new(store: &'s FirStore, module_id: FirId) -> Self {
        Self {
            store,
            module_id,
            diags: Vec::new(),
            symbols: ModuleSymbols::default(),
            current_function: None,
            current_return_type: None,
            current_fun_args: HashMap::new(),
            scope_stack: ScopeStack::new(),
            sub_module_names: HashSet::new(),
        }
    }
    /// Appends one diagnostic enriched with current function context.
    pub(super) fn emit(
        &mut self,
        severity: Severity,
        code: &'static str,
        message: impl Into<String>,
        node: FirId,
    ) {
        self.diags.push(FirDiagnostic {
            severity,
            code,
            message: message.into(),
            node,
            context: DiagContext {
                function_name: self.current_function.clone(),
                variable_name: None,
            },
        });
    }
    /// Convenience helper for emitting an error diagnostic.
    pub(super) fn error(&mut self, code: &'static str, message: impl Into<String>, node: FirId) {
        self.emit(Severity::Error, code, message, node);
    }
    /// Convenience helper for emitting a warning diagnostic.
    pub(super) fn warn(&mut self, code: &'static str, message: impl Into<String>, node: FirId) {
        self.emit(Severity::Warning, code, message, node);
    }
    /// Initializes per-function verification state and seeds `kFunArgs`.
    pub(super) fn enter_function(
        &mut self,
        name: String,
        ret: FirType,
        args: impl Iterator<Item = (String, FirType)>,
    ) {
        self.current_function = Some(name);
        self.current_return_type = Some(ret);
        self.current_fun_args.clear();
        for (param_name, param_type) in args {
            self.current_fun_args.insert(param_name, param_type);
        }
        self.scope_stack.push(FrameKind::Function);
    }
    /// Clears per-function verification state after a body traversal.
    pub(super) fn leave_function(&mut self) {
        self.scope_stack.pop();
        self.current_function = None;
        self.current_return_type = None;
        self.current_fun_args.clear();
    }
    /// Resolve a variable name+access to its declared `VarEntry`, if any.
    ///
    /// Returns `None` only when the variable is genuinely undeclared.
    ///
    /// `kStruct` accesses are validated against names declared in the
    /// `dsp_struct` block. The returned type remains a placeholder because
    /// checker phase 3 still relies on the explicit FIR node `typ` for struct
    /// accesses (name→type mapping is not tracked yet).
    pub(super) fn resolve(&self, name: &str, access: AccessType) -> Option<VarEntry> {
        match access {
            AccessType::Struct => {
                if !self.symbols.struct_field_names.contains(name) {
                    return None;
                }
                Some(VarEntry {
                    access: AccessType::Struct,
                    typ: FirType::Void, // placeholder; type check is Phase 3
                    init: InitStatus::Yes,
                    is_table: false,
                })
            }
            AccessType::Static | AccessType::Global => {
                let (a, t) = self.symbols.globals.get(name)?;
                Some(VarEntry {
                    access: *a,
                    typ: t.clone(),
                    init: InitStatus::Yes,
                    is_table: self.symbols.global_tables.contains(name),
                })
            }
            AccessType::FunArgs => {
                let t = self.current_fun_args.get(name)?;
                Some(VarEntry {
                    access: AccessType::FunArgs,
                    typ: t.clone(),
                    init: InitStatus::Yes,
                    is_table: false,
                })
            }
            AccessType::Stack | AccessType::Loop => {
                let (_, e) = self.scope_stack.lookup(name)?;
                Some(e.clone())
            }
        }
    }
    /// Resolve a variable by name only (ignoring the requested access class).
    ///
    /// Used to distinguish "undeclared" from "declared in another access space"
    /// so SC02/SC05 can be emitted instead of SC01/SC04.
    pub(super) fn resolve_any_by_name(&self, name: &str) -> Option<VarEntry> {
        if let Some((_, entry)) = self.scope_stack.lookup(name) {
            return Some(entry.clone());
        }
        if let Some(t) = self.current_fun_args.get(name) {
            return Some(VarEntry {
                access: AccessType::FunArgs,
                typ: t.clone(),
                init: InitStatus::Yes,
                is_table: false,
            });
        }
        if let Some((access, typ)) = self.symbols.globals.get(name) {
            return Some(VarEntry {
                access: *access,
                typ: typ.clone(),
                init: InitStatus::Yes,
                is_table: self.symbols.global_tables.contains(name),
            });
        }
        if self.symbols.struct_field_names.contains(name) {
            return Some(VarEntry {
                access: AccessType::Struct,
                typ: FirType::Void,
                init: InitStatus::Yes,
                is_table: false,
            });
        }
        None
    }
}
