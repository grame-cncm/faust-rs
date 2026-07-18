//! FIR module verifier — Phase 1, Phase 2, and Phase 3.
//!
//! **Phase 1** validates the top-level shape of a `FirMatch::Module` node and
//! populates [`ModuleSymbols`] (struct fields, globals, declared functions).
//!
//! **Phase 2** traverses every function body and performs scope analysis:
//! variable declarations, accesses, loop structures, return statements, and
//! switch statements.
//!
//! **Phase 3** is implemented on top of the same traversal and adds expression
//! type checks (binops, casts, select, function calls, table accesses, and
//! typed control-flow conditions).
//!
//! # Diagnostic codes implemented
//!
//! ## Phase 1 — module structure
//! | Code | Sev | Check |
//! |---|---|---|
//! | FIR-M01 | E | Root node is not a Module |
//! | FIR-M02 | E | `dsp_struct` is not a `Block` of struct field declarations |
//! | FIR-M03 | E | `globals` is not a Block |
//! | FIR-M04 | E | `functions` is not a Block |
//! | FIR-M05 | E | Non-`DeclareFun` node in functions block |
//! | FIR-M06 | W | Duplicate function name in functions |
//! | FIR-M07 | W | Expected DSP API function is not declared |
//! | FIR-S01 | E | Struct field declaration is not `DeclareVar/DeclareTable(kStruct)` |
//! | FIR-S02 | E | Duplicate struct field name in `dsp_struct` |
//! | FIR-S03 | E | Struct field has `Void` type |
//! | FIR-S04 | W | Struct array field has size 0 |
//! | FIR-G01 | E | Globals block contains a non-`DeclareVar`/`DeclareTable`/`DeclareFun` node |
//! | FIR-G02 | E | Global declaration has wrong access type |
//! | FIR-G03 | E | Duplicate global variable name |
//! | FIR-F01 | E | Function type is not `FirType::Fun` |
//! | FIR-F04 | E | Duplicate parameter name in function |
//! | FIR-F05 | W | `compute` return type is not `Void` |
//! | FIR-F06 | W | `compute` parameter count is not 4 |
//! | FIR-F07 | W | Function has no body (prototype/extern declaration) |
//!
//! ## Phase 2 — per-function scope analysis
//! | Code | Sev | Check |
//! |---|---|---|
//! | FIR-LC01 | E | `LoadVar(kStruct)` in `instanceConstants` reads a field only initialized in `instanceClear` |
//! | FIR-SC01 | E | `LoadVar` of undeclared variable |
//! | FIR-SC02 | E | `LoadVar` access type does not match declaration |
//! | FIR-SC03 | W | `LoadVar` of uninitialized stack variable |
//! | FIR-SC04 | E | `StoreVar` to undeclared variable |
//! | FIR-SC05 | E | `StoreVar` access type does not match declaration |
//! | FIR-SC07 | E | `kFunArgs` variable re-declared inside function body |
//! | FIR-SC09 | W | `kStruct` access name not declared in `dsp_struct` |
//! | FIR-SC10 | E | Local `DeclareVar` uses a non-local access class |
//! | FIR-L01  | E | `ForLoop` init is not a `DeclareVar(kLoop)` |
//! | FIR-L02  | E | `ForLoop` loop variable type is not `Int32`/`Int64` |
//! | FIR-L04  | W | `ForLoop`/`SimpleForLoop` body is empty |
//! | FIR-R02  | W | `Return(None)` in a non-`Void` function |
//! | FIR-R03  | W | Statements after a `Return` in a block (dead code) |
//! | FIR-SW02 | E | Duplicate case value in `Switch` |
//! | FIR-SW03 | W | `Switch` has no cases |
//!
//! ## Phase 3 — type checking and typed conditions
//! | Code | Sev | Check |
//! |---|---|---|
//! | FIR-B01 | E | `BinOp` operand type mismatch (except int/bool mixing) |
//! | FIR-B02 | E | `BinOp` operand is not numeric |
//! | FIR-B03 | W | `BinOp` declared result type inconsistent with operands |
//! | FIR-B04 | W | Division by constant zero |
//! | FIR-U01 | E | `Neg` operand is not numeric |
//! | FIR-U02 | W | `Cast` is a no-op |
//! | FIR-U03 | E | `Cast` between non-numeric types |
//! | FIR-U04 | W | `Bitcast` width mismatch |
//! | FIR-C01 | E | `Select2` condition is not int/bool |
//! | FIR-C02 | W | `Select2` branch type mismatch |
//! | FIR-C03 | W | `Select2` result type inconsistent with branches |
//! | FIR-C04 | E | `If` condition is not int/bool |
//! | FIR-FC01 | E | Call to undeclared function |
//! | FIR-FC02 | E | Function call arity mismatch |
//! | FIR-FC03 | W | Function call argument type mismatch |
//! | FIR-FC04 | W | Function return value type mismatch at use site (partial) |
//! | FIR-L03  | E | `WhileLoop` condition is not int/bool |
//! | FIR-SW01 | E | `Switch` condition is not integer |
//! | FIR-R01  | E | `Return` value type mismatch |
//! | FIR-T01  | E | Table index is not integer |
//! | FIR-T02  | E | `StoreTable` value type mismatch |
//! | FIR-T03  | W | `LoadTable` / `StoreTable` on non-table declaration |
//! | FIR-T04  | E | `LoadTable` / `StoreTable` name has no declaration at all |
//! | FIR-SF01 | W | Soundfile access refers to a non-`Sound` struct field |
//! | FIR-MA01 | W | Unary math op called with wrong arity |
//! | FIR-MA02 | W | Binary math op called with wrong arity |
//! | FIR-MA03 | W | Floating-point math op called with integer-like argument |
//! | FIR-MA04 | W | `abs` / `fabs` int-vs-float distinction warning |
//! | FIR-V01  | E | `Void`-typed expression used where a material value is required |
//!
//! ## Deferred / partial
//! - **SC06/SC08** — naturally enforced by scope-stack pop; SC01 fires for any
//!   out-of-scope access regardless of the access class.
//! - **FC04** — implemented partially (discarded non-void call result + call
//!   node declared-type/signature mismatch), but not yet all assignment/use
//!   sites.
//!
//! # Source provenance
//! - Plan: `porting/fir-module-verifier-plan-en.md`, §7
//! - C++ parity: `FIRTypeChecker`, `FIRCodeChecker`, `FIRVarChecker`
//!   in `compiler/generator/fir_to_fir.hh` and `fir_code_checker.hh`

use std::collections::{HashMap, HashSet};

use crate::{
    AccessType, FirBinOp, FirId, FirMatch, FirMathOp, FirStore, FirType, NamedType, child_ids,
    match_fir,
};

// ─── Diagnostic types ─────────────────────────────────────────────────────────

/// Severity of a verifier diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// A blocking FIR invariant violation.
    Error,
    /// A non-blocking but suspicious FIR pattern.
    Warning,
}

/// A single diagnostic produced during FIR verification.
#[derive(Clone, Debug, PartialEq)]
pub struct FirDiagnostic {
    /// Diagnostic severity (`Error` or `Warning`).
    pub severity: Severity,
    /// Short code from the diagnostic registry, e.g. `"FIR-M01"`.
    pub code: &'static str,
    /// Human-readable diagnostic message.
    pub message: String,
    /// The [`FirId`] most closely associated with the problem.
    pub node: FirId,
    /// Optional contextual metadata (current function, variable, ...).
    pub context: DiagContext,
}

/// Contextual location of a diagnostic (enclosing function, variable, etc.).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiagContext {
    /// Enclosing function name when the diagnostic originates in a function body.
    pub function_name: Option<String>,
    /// Variable name when the checker can identify a specific variable symbol.
    pub variable_name: Option<String>,
}

// ─── Verify report ─────────────────────────────────────────────────────────────

/// Result of a FIR verification run.
///
/// The verifier is diagnostic-first: callers receive the full report and can
/// decide whether warnings are acceptable for their pipeline stage.
#[derive(Debug, Default)]
pub struct FirVerifyReport {
    /// All diagnostics emitted during the verification run.
    pub diagnostics: Vec<FirDiagnostic>,
}

impl FirVerifyReport {
    /// Returns `true` if any `Error`-severity diagnostics were emitted.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Iterates over error-severity diagnostics.
    pub fn errors(&self) -> impl Iterator<Item = &FirDiagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
    }

    /// Iterates over warning-severity diagnostics.
    pub fn warnings(&self) -> impl Iterator<Item = &FirDiagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
    }

    /// Panics with a formatted listing of all errors if any are present.
    ///
    /// Intended for use in tests and debug-build assertions.
    pub fn assert_ok(&self) {
        if self.has_errors() {
            let msgs = self
                .errors()
                .map(|d| format!("  [{}] {}", d.code, d.message))
                .collect::<Vec<_>>()
                .join("\n");
            panic!("FIR verification failed:\n{msgs}");
        }
    }
}

// ─── Module symbol tables ──────────────────────────────────────────────────────

/// Signature of a function declared in the module.
///
/// This is the distilled function view used by later phases; it intentionally
/// stores only the information needed for scope/type checks, not the full FIR
/// declaration node.
#[derive(Clone, Debug)]
pub struct FunctionSig {
    /// Ordered list of `(param_name, param_type)` pairs.
    pub params: Vec<(String, FirType)>,
    /// Return type from the function signature.
    pub return_type: FirType,
    /// `true` when the function has no body (prototype / extern declaration).
    pub is_extern: bool,
}

/// Symbol tables populated during Phase 1 (module-level pass).
///
/// These tables feed into Phase 2 (scope analysis) and Phase 3 (type checking).
/// They form the verifier's canonical summary of module-level declarations.
#[derive(Clone, Debug, Default)]
pub struct ModuleSymbols {
    /// Logical DSP struct name (currently sourced from `Module.name`).
    pub struct_name: Option<String>,
    /// Ordered field types from declarations in the `dsp_struct` block.
    ///
    /// Field names are tracked separately in [`Self::struct_field_names`].
    pub struct_fields: Vec<FirType>,
    /// Set of names declared in the `dsp_struct` block (vars and tables).
    pub struct_field_names: HashSet<String>,
    /// Struct field types keyed by field name.
    pub struct_field_types: HashMap<String, FirType>,
    /// Global/static variables: name → `(AccessType, FirType)`.
    pub globals: HashMap<String, (AccessType, FirType)>,
    /// Names declared as global/static tables (for T03).
    ///
    /// This is tracked separately because table-ness is not encoded in
    /// [`globals`](Self::globals) (which stores only access + element type).
    pub global_tables: HashSet<String>,
    /// Declared functions: name → [`FunctionSig`].
    pub functions: HashMap<String, FunctionSig>,
}

// ─── DSP API registry ──────────────────────────────────────────────────────────

/// Expected DSP API function names checked by M07.
pub const DSP_API_FUNCTIONS: &[&str] = &[
    "classInit",
    "instanceConstants",
    "instanceResetUserInterface",
    "instanceClear",
    "instanceInit",
    "init",
    "buildUserInterface",
    "getSampleRate",
    "compute",
    "metadata",
];

// ─── Phase 2 — scope analysis types ───────────────────────────────────────────

/// Initialization status of a local variable.
///
/// This is a lightweight definite-initialization lattice used by the scope/type
/// traversal. It is intentionally coarse: the verifier tracks obvious missing
/// writes without attempting full dataflow precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitStatus {
    /// Variable declared but not yet assigned.
    No,
    /// Variable definitely assigned on all paths so far.
    Yes,
    /// Variable assigned on some but not all control-flow paths.
    Maybe,
}

/// Entry stored in a [`ScopeFrame`] for each declared variable.
#[derive(Clone, Debug)]
struct VarEntry {
    /// Access class used by loads/stores to this symbol.
    access: AccessType,
    /// FIR type declared for the symbol (element type for tables).
    typ: FirType,
    /// Definite-initialization state tracked by Phase 2 control-flow analysis.
    init: InitStatus,
    /// `true` when the symbol was declared as a table (`DeclareTable`).
    is_table: bool,
}

/// Kind of a scope frame.
///
/// The kind is carried at push sites to keep scope-manipulation call sites
/// self-documenting and to leave room for future kind-specific checks even
/// though the current stack stores bindings only.
#[derive(Clone, Debug)]
enum FrameKind {
    /// Ordinary `Block`.
    Block,
    /// Loop body (ForLoop / SimpleForLoop / IteratorForLoop).
    Loop,
    /// Top-level function frame (holds kFunArgs pre-populated).
    Function,
}

/// One level of the lexical scope stack.
///
/// Frames are intentionally minimal and only store bindings; higher-level
/// traversal context (loop/function meaning) stays with the caller.
#[derive(Clone, Debug)]
struct ScopeFrame {
    /// Variables declared in this lexical frame.
    vars: HashMap<String, VarEntry>,
}

/// Lexical scope stack for Phase 2 traversal.
///
/// Lookup walks from innermost to outermost frame, matching the shadowing rules
/// expected by FIR function bodies after earlier lowering passes.
struct ScopeStack {
    /// Stack of lexical frames from outermost to innermost.
    frames: Vec<ScopeFrame>,
}

impl ScopeStack {
    /// Creates an empty lexical scope stack.
    fn new() -> Self {
        Self { frames: Vec::new() }
    }

    /// Pushes a new lexical frame.
    ///
    /// `FrameKind` is currently carried by callers for readability and future
    /// extensions; the stack stores only the frame bindings.
    fn push(&mut self, _kind: FrameKind) {
        self.frames.push(ScopeFrame {
            vars: HashMap::new(),
        });
    }

    /// Pops the current lexical frame.
    fn pop(&mut self) {
        self.frames.pop();
    }

    /// Declare a variable in the current (top) frame.
    fn declare(&mut self, name: String, typ: FirType, access: AccessType, init: InitStatus) {
        self.declare_with_kind(name, typ, access, init, false);
    }

    /// Declare a table-like symbol in the current frame.
    ///
    /// Local tables are considered initialized at declaration time.
    fn declare_table(&mut self, name: String, elem_type: FirType, access: AccessType) {
        self.declare_with_kind(name, elem_type, access, InitStatus::Yes, true);
    }

    /// Shared insertion helper for variable/table declarations.
    fn declare_with_kind(
        &mut self,
        name: String,
        typ: FirType,
        access: AccessType,
        init: InitStatus,
        is_table: bool,
    ) {
        if let Some(frame) = self.frames.last_mut() {
            frame.vars.insert(
                name,
                VarEntry {
                    access,
                    typ,
                    init,
                    is_table,
                },
            );
        }
    }

    /// Look up a variable from the top of the stack downward.
    /// Returns `(frame_index, &VarEntry)` or `None`.
    fn lookup(&self, name: &str) -> Option<(usize, &VarEntry)> {
        for (fi, frame) in self.frames.iter().enumerate().rev() {
            if let Some(entry) = frame.vars.get(name) {
                return Some((fi, entry));
            }
        }
        None
    }

    /// Mark a variable as initialized (update the topmost frame that holds it).
    fn mark_initialized(&mut self, name: &str) {
        for frame in self.frames.iter_mut().rev() {
            if let Some(entry) = frame.vars.get_mut(name) {
                entry.init = InitStatus::Yes;
                return;
            }
        }
    }

    // ── Snapshot / restore for If-branch merge ──────────────────────────────

    /// Snapshot the current `InitStatus` of every variable in every frame.
    fn snapshot_inits(&self) -> Vec<(usize, String, InitStatus)> {
        self.frames
            .iter()
            .enumerate()
            .flat_map(|(fi, frame)| {
                frame
                    .vars
                    .iter()
                    .map(move |(name, entry)| (fi, name.clone(), entry.init))
            })
            .collect()
    }

    /// Restore `InitStatus` values from a previous snapshot (does not add or
    /// remove variables, only resets existing init flags).
    fn restore_inits(&mut self, snap: &[(usize, String, InitStatus)]) {
        for (fi, name, status) in snap {
            if let Some(frame) = self.frames.get_mut(*fi)
                && let Some(entry) = frame.vars.get_mut(name.as_str())
            {
                entry.init = *status;
            }
        }
    }

    /// Returns the set of `(frame_idx, var_name)` pairs whose `InitStatus`
    /// changed compared to `snap` (i.e. were newly initialized in the branch).
    fn diff_inits(&self, snap: &[(usize, String, InitStatus)]) -> Vec<(usize, String)> {
        snap.iter()
            .filter_map(|(fi, name, old)| {
                let cur = self
                    .frames
                    .get(*fi)
                    .and_then(|f| f.vars.get(name.as_str()))
                    .map(|e| e.init);
                if cur != Some(*old) {
                    Some((*fi, name.clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Set the `InitStatus` of a specific `(frame_idx, var_name)` pair.
    fn set_init(&mut self, fi: usize, name: &str, status: InitStatus) {
        if let Some(frame) = self.frames.get_mut(fi)
            && let Some(entry) = frame.vars.get_mut(name)
        {
            entry.init = status;
        }
    }
}

// ─── Entry points ─────────────────────────────────────────────────────────────

/// Verify the FIR module (Phase 1 + Phase 2 + Phase 3) and return the diagnostic report.
///
/// This is the main verifier entry point used by tests, pass assertions, and
/// compiler integration. It validates the module shape, collects top-level
/// symbols, then walks all function bodies for scope and type checks.
#[must_use]
pub fn verify_fir_module(store: &FirStore, module_id: FirId) -> FirVerifyReport {
    let (report, _symbols) = verify_module_structure(store, module_id);
    report
}

/// Like [`verify_fir_module`] but also returns the [`ModuleSymbols`] collected
/// during Phase 1 for targeted function verification or later passes.
///
/// The returned [`FirVerifyReport`] already includes all diagnostics emitted by
/// phase 1, phase 2, and phase 3.
#[must_use]
pub fn verify_module_structure(
    store: &FirStore,
    module_id: FirId,
) -> (FirVerifyReport, ModuleSymbols) {
    let mut ctx = VerifyCtx::new(store, module_id);
    ctx.check_phase1();
    ctx.check_phase2();
    (
        FirVerifyReport {
            diagnostics: ctx.diags,
        },
        ctx.symbols,
    )
}

/// Verify a single function body using pre-collected module symbols.
///
/// This runs the per-function Phase 2 + Phase 3 semantic checks (no module-shape
/// validation).
///
/// If `fun_id` is not a `DeclareFun` node (or its type is not `FirType::Fun`),
/// a diagnostic is emitted in the returned report.
#[must_use]
pub fn verify_fir_function(
    store: &FirStore,
    fun_id: FirId,
    symbols: &ModuleSymbols,
) -> FirVerifyReport {
    let mut ctx = VerifyCtx::new(store, fun_id);
    ctx.symbols = symbols.clone();

    let FirMatch::DeclareFun {
        name,
        typ,
        args,
        body,
        ..
    } = match_fir(store, fun_id)
    else {
        ctx.error("FIR-M05", "node is not a DeclareFun", fun_id);
        return FirVerifyReport {
            diagnostics: ctx.diags,
        };
    };

    let FirType::Fun { ret, .. } = typ else {
        ctx.error(
            "FIR-F01",
            format!("function '{name}' has type that is not FirType::Fun"),
            fun_id,
        );
        return FirVerifyReport {
            diagnostics: ctx.diags,
        };
    };

    if let Some(body_id) = body {
        ctx.enter_function(
            name,
            *ret,
            args.iter().map(|a| (a.name.clone(), a.typ.clone())),
        );
        ctx.check_stmt(body_id);
        ctx.leave_function();
    }

    FirVerifyReport {
        diagnostics: ctx.diags,
    }
}

// ─── Internal context ──────────────────────────────────────────────────────────

/// Mutable verifier state shared by all verification phases.
///
/// This context intentionally centralizes diagnostics, symbol tables, and the
/// per-function scope/type state so phase ordering remains explicit and tests
/// can exercise the same engine through both module-level and function-level
/// entry points.
struct VerifyCtx<'s> {
    /// FIR storage containing all nodes referenced by the verifier.
    store: &'s FirStore,
    /// Root module (or function in single-function mode) currently verified.
    module_id: FirId,
    /// Collected diagnostics in emission order.
    diags: Vec<FirDiagnostic>,
    /// Module symbols collected/consumed across verification phases.
    symbols: ModuleSymbols,

    // ── Phase 2 per-function state ─────────────────────────────────────────
    /// Name of the function currently being verified.
    current_function: Option<String>,
    /// Return type of the function currently being verified.
    current_return_type: Option<FirType>,
    /// `kFunArgs` parameters of the current function: name → type.
    current_fun_args: HashMap<String, FirType>,
    /// Lexical scope stack for `kStack` / `kLoop` variables.
    scope_stack: ScopeStack,
}

impl<'s> VerifyCtx<'s> {
    /// Creates a new verifier context rooted at `module_id`.
    ///
    /// For full-module verification `module_id` is the FIR `Module` node; for
    /// single-function verification it may temporarily be a `DeclareFun`.
    fn new(store: &'s FirStore, module_id: FirId) -> Self {
        Self {
            store,
            module_id,
            diags: Vec::new(),
            symbols: ModuleSymbols::default(),
            current_function: None,
            current_return_type: None,
            current_fun_args: HashMap::new(),
            scope_stack: ScopeStack::new(),
        }
    }

    /// Appends one diagnostic enriched with current function context.
    fn emit(
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
    fn error(&mut self, code: &'static str, message: impl Into<String>, node: FirId) {
        self.emit(Severity::Error, code, message, node);
    }

    /// Convenience helper for emitting a warning diagnostic.
    fn warn(&mut self, code: &'static str, message: impl Into<String>, node: FirId) {
        self.emit(Severity::Warning, code, message, node);
    }

    // =========================================================================
    // Phase 1 — module structure and symbol collection
    // =========================================================================

    /// Runs Phase 1 module checks and collects top-level symbols.
    ///
    /// This validates the `Module` skeleton (`dsp_struct`, `globals`,
    /// `functions`) and populates symbol tables consumed by phases 2/3.
    fn check_phase1(&mut self) {
        let id = self.module_id;

        // M01: root must decode as Module
        let FirMatch::Module {
            num_inputs,
            num_outputs,
            name,
            dsp_struct,
            globals,
            functions,
            static_decls,
        } = match_fir(self.store, id)
        else {
            self.error("FIR-M01", "root node is not a FirMatch::Module", id);
            return;
        };

        // `Module.name` is the DSP class name and is used as the logical struct
        // name in the checker symbols table.
        self.symbols.struct_name = Some(name.clone());
        // M02: validate and register struct fields
        self.check_dsp_struct(dsp_struct);

        // M03: globals must be a Block
        match match_fir(self.store, globals) {
            FirMatch::Block(stmts) => self.check_globals(globals, stmts),
            _ => self.error("FIR-M03", "globals is not a Block", globals),
        }

        // M04: functions must be a Block
        match match_fir(self.store, functions) {
            FirMatch::Block(stmts) => {
                self.check_functions(functions, stmts, &name);
                self.check_compute_io_arity_contract(functions, num_inputs, num_outputs);
            }
            _ => self.error("FIR-M04", "functions is not a Block", functions),
        }

        // M05: static_decls must be a Block of Static/Global table declarations.
        // Reuse check_globals — it already validates AccessType::Static and
        // registers names into symbols.globals so that load_table(Static)
        // accesses in compute resolve correctly.
        match match_fir(self.store, static_decls) {
            FirMatch::Block(stmts) => self.check_globals(static_decls, stmts),
            _ => self.error("FIR-M05", "static_decls is not a Block", static_decls),
        }
    }

    /// Checks that `compute` body aliases and `inputs[]`/`outputs[]` indices
    /// stay within the module-level `(num_inputs, num_outputs)` contract.
    fn check_compute_io_arity_contract(
        &mut self,
        functions: FirId,
        num_inputs: usize,
        num_outputs: usize,
    ) {
        let FirMatch::Block(items) = match_fir(self.store, functions) else {
            return;
        };
        for item in items {
            let FirMatch::DeclareFun {
                name,
                body: Some(body),
                ..
            } = match_fir(self.store, item)
            else {
                continue;
            };
            if name != "compute" {
                continue;
            }
            self.check_compute_body_io_access(body, num_inputs, num_outputs);
            break;
        }
    }

    /// Recursively walks the `compute` body and emits:
    /// - `FIR-M08` when an input alias/index exceeds `num_inputs`
    /// - `FIR-M09` when an output alias/index exceeds `num_outputs`.
    fn check_compute_body_io_access(&mut self, id: FirId, num_inputs: usize, num_outputs: usize) {
        match match_fir(self.store, id) {
            FirMatch::Block(items) => {
                for stmt in items {
                    self.check_compute_body_io_access(stmt, num_inputs, num_outputs);
                }
            }
            FirMatch::DeclareVar { name, init, .. } => {
                if let Some(index) = input_alias_index(name.as_str())
                    && index >= num_inputs
                {
                    self.error(
                        "FIR-M08",
                        format!(
                            "compute declares input alias '{name}' out of module input arity {num_inputs}"
                        ),
                        id,
                    );
                }
                if let Some(index) = output_alias_index(name.as_str())
                    && index >= num_outputs
                {
                    self.error(
                        "FIR-M09",
                        format!(
                            "compute declares output alias '{name}' out of module output arity {num_outputs}"
                        ),
                        id,
                    );
                }
                if let Some(init) = init {
                    self.check_compute_body_io_access(init, num_inputs, num_outputs);
                }
            }
            FirMatch::LoadTable {
                name,
                access: AccessType::FunArgs,
                index,
                ..
            } => {
                if let Some(index) = funargs_constant_index(self.store, index) {
                    if name == "inputs" && index >= num_inputs {
                        self.error(
                            "FIR-M08",
                            format!(
                                "compute reads inputs[{index}] but module has {num_inputs} inputs"
                            ),
                            id,
                        );
                    } else if name == "outputs" && index >= num_outputs {
                        self.error(
                            "FIR-M09",
                            format!(
                                "compute reads outputs[{index}] but module has {num_outputs} outputs"
                            ),
                            id,
                        );
                    }
                }
                self.check_compute_body_io_access(index, num_inputs, num_outputs);
            }
            FirMatch::StoreTable { index, value, .. } => {
                self.check_compute_body_io_access(index, num_inputs, num_outputs);
                self.check_compute_body_io_access(value, num_inputs, num_outputs);
            }
            FirMatch::SimpleForLoop { upper, body, .. } => {
                self.check_compute_body_io_access(upper, num_inputs, num_outputs);
                self.check_compute_body_io_access(body, num_inputs, num_outputs);
            }
            FirMatch::ForLoop {
                init,
                end,
                step,
                body,
                ..
            } => {
                self.check_compute_body_io_access(init, num_inputs, num_outputs);
                self.check_compute_body_io_access(end, num_inputs, num_outputs);
                self.check_compute_body_io_access(step, num_inputs, num_outputs);
                self.check_compute_body_io_access(body, num_inputs, num_outputs);
            }
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                self.check_compute_body_io_access(cond, num_inputs, num_outputs);
                self.check_compute_body_io_access(then_block, num_inputs, num_outputs);
                if let Some(else_block) = else_block {
                    self.check_compute_body_io_access(else_block, num_inputs, num_outputs);
                }
            }
            FirMatch::Control { cond, stmt } => {
                self.check_compute_body_io_access(cond, num_inputs, num_outputs);
                self.check_compute_body_io_access(stmt, num_inputs, num_outputs);
            }
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => {
                self.check_compute_body_io_access(cond, num_inputs, num_outputs);
                for (_, block) in cases {
                    self.check_compute_body_io_access(block, num_inputs, num_outputs);
                }
                if let Some(default) = default {
                    self.check_compute_body_io_access(default, num_inputs, num_outputs);
                }
            }
            FirMatch::WhileLoop { cond, body } => {
                self.check_compute_body_io_access(cond, num_inputs, num_outputs);
                self.check_compute_body_io_access(body, num_inputs, num_outputs);
            }
            FirMatch::BinOp { lhs, rhs, .. } => {
                self.check_compute_body_io_access(lhs, num_inputs, num_outputs);
                self.check_compute_body_io_access(rhs, num_inputs, num_outputs);
            }
            FirMatch::Neg { value, .. }
            | FirMatch::Cast { value, .. }
            | FirMatch::Bitcast { value, .. }
            | FirMatch::StoreVar { value, .. }
            | FirMatch::TeeVar { value, .. }
            | FirMatch::Drop(value)
            | FirMatch::Return(Some(value)) => {
                self.check_compute_body_io_access(value, num_inputs, num_outputs);
            }
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                ..
            } => {
                self.check_compute_body_io_access(cond, num_inputs, num_outputs);
                self.check_compute_body_io_access(then_value, num_inputs, num_outputs);
                self.check_compute_body_io_access(else_value, num_inputs, num_outputs);
            }
            FirMatch::FunCall { args, .. } | FirMatch::ValueArray { values: args, .. } => {
                for arg in args {
                    self.check_compute_body_io_access(arg, num_inputs, num_outputs);
                }
            }
            FirMatch::DeclareTable { values, .. } => {
                for value in values {
                    self.check_compute_body_io_access(value, num_inputs, num_outputs);
                }
            }
            _ => {}
        }
    }

    // ── dsp_struct ────────────────────────────────────────────────────────────

    /// Validates `dsp_struct` layout and records declared struct field names.
    fn check_dsp_struct(&mut self, id: FirId) {
        let FirMatch::Block(stmts) = match_fir(self.store, id) else {
            self.error("FIR-M02", "dsp_struct is not a Block", id);
            return;
        };

        let mut seen = HashSet::new();
        let mut field_types_by_name = HashMap::new();
        let mut field_types = Vec::new();
        for stmt_id in stmts {
            let (field_name, field_type) = match match_fir(self.store, stmt_id) {
                FirMatch::DeclareVar {
                    name, typ, access, ..
                } => {
                    if access != AccessType::Struct {
                        self.error(
                            "FIR-S01",
                            format!(
                                "dsp_struct field '{name}' has access type {access:?}, expected Struct"
                            ),
                            stmt_id,
                        );
                    }
                    (name, typ)
                }
                FirMatch::DeclareTable {
                    name,
                    access,
                    elem_type,
                    ..
                } => {
                    if access != AccessType::Struct {
                        self.error(
                            "FIR-S01",
                            format!(
                                "dsp_struct table '{name}' has access type {access:?}, expected Struct"
                            ),
                            stmt_id,
                        );
                    }
                    (name, elem_type)
                }
                _ => {
                    self.error(
                        "FIR-S01",
                        "dsp_struct contains a node that is not DeclareVar or DeclareTable",
                        stmt_id,
                    );
                    continue;
                }
            };

            if !seen.insert(field_name.clone()) {
                self.error(
                    "FIR-S02",
                    format!("duplicate dsp_struct field name '{field_name}'"),
                    stmt_id,
                );
            }

            match &field_type {
                FirType::Void => {
                    self.error(
                        "FIR-S03",
                        format!("dsp_struct field '{field_name}' has Void type"),
                        stmt_id,
                    );
                }
                FirType::Array(_, 0) => {
                    self.warn(
                        "FIR-S04",
                        format!("dsp_struct array field '{field_name}' has size 0"),
                        stmt_id,
                    );
                }
                _ => {}
            }
            field_types_by_name.insert(field_name.clone(), field_type.clone());
            field_types.push(field_type);
        }

        self.symbols.struct_field_names = seen;
        self.symbols.struct_field_types = field_types_by_name;
        self.symbols.struct_fields = field_types;
    }

    // ── globals ───────────────────────────────────────────────────────────────

    /// Validates `globals` declarations and registers global symbols/functions.
    ///
    /// `globals` may contain variable/table declarations and prototype-only
    /// `DeclareFun` externs (for example math functions used by FIR calls).
    fn check_globals(&mut self, _block_id: FirId, stmts: Vec<FirId>) {
        let mut seen: HashSet<String> = HashSet::new();

        for stmt_id in stmts {
            match match_fir(self.store, stmt_id) {
                FirMatch::DeclareVar {
                    name, typ, access, ..
                } => {
                    if !matches!(access, AccessType::Static | AccessType::Global) {
                        self.error(
                            "FIR-G02",
                            format!(
                                "global variable '{name}' has access type {access:?}, \
                                 expected Static or Global"
                            ),
                            stmt_id,
                        );
                    }
                    if seen.insert(name.clone()) {
                        self.symbols.globals.insert(name, (access, typ));
                    } else {
                        self.error(
                            "FIR-G03",
                            format!("duplicate global variable name '{name}'"),
                            stmt_id,
                        );
                    }
                }
                FirMatch::DeclareTable {
                    name,
                    access,
                    elem_type,
                    ..
                } => {
                    if !matches!(access, AccessType::Static | AccessType::Global) {
                        self.error(
                            "FIR-G02",
                            format!(
                                "global table '{name}' has access type {access:?}, \
                                 expected Static or Global"
                            ),
                            stmt_id,
                        );
                    }
                    if seen.insert(name.clone()) {
                        self.symbols.global_tables.insert(name.clone());
                        self.symbols.globals.insert(name, (access, elem_type));
                    } else {
                        self.error(
                            "FIR-G03",
                            format!("duplicate global variable name '{name}'"),
                            stmt_id,
                        );
                    }
                }
                FirMatch::DeclareFun {
                    name,
                    typ,
                    args,
                    body,
                    ..
                } => {
                    self.register_function_signature(
                        stmt_id, &name, &typ, &args, body, None, false,
                    );
                }
                _ => {
                    self.error(
                        "FIR-G01",
                        "globals block contains a node that is not DeclareVar, DeclareTable, or DeclareFun",
                        stmt_id,
                    );
                }
            }
        }
    }

    // ── functions ─────────────────────────────────────────────────────────────

    /// Validates the module `functions` block and registers all signatures.
    fn check_functions(&mut self, _block_id: FirId, stmts: Vec<FirId>, _module_name: &str) {
        let mut seen: HashSet<String> = HashSet::new();

        for stmt_id in stmts {
            let FirMatch::DeclareFun {
                name,
                typ,
                args,
                body,
                ..
            } = match_fir(self.store, stmt_id)
            else {
                self.error(
                    "FIR-M05",
                    "functions block contains a non-DeclareFun node",
                    stmt_id,
                );
                continue;
            };

            self.register_function_signature(
                stmt_id,
                &name,
                &typ,
                &args,
                body,
                Some(&mut seen),
                true,
            );
        }

        for &api_fn in DSP_API_FUNCTIONS {
            if !self.symbols.functions.contains_key(api_fn) {
                self.warn(
                    "FIR-M07",
                    format!("expected DSP API function '{api_fn}' is not declared"),
                    self.module_id,
                );
            }
        }
    }

    /// Validates one `DeclareFun` signature and stores it in `symbols.functions`.
    ///
    /// This helper is shared by `globals` (extern prototypes) and
    /// `functions` (regular function declarations). It validates function
    /// signature shape, records signatures into `symbols.functions`, and
    /// optionally tracks duplicate names in `seen_names`.
    #[allow(clippy::too_many_arguments)]
    fn register_function_signature(
        &mut self,
        stmt_id: FirId,
        name: &str,
        typ: &FirType,
        args: &[NamedType],
        body: Option<FirId>,
        seen_names: Option<&mut HashSet<String>>,
        warn_extern: bool,
    ) {
        if let Some(seen) = seen_names
            && !seen.insert(name.to_string())
        {
            self.warn(
                "FIR-M06",
                format!("duplicate function name '{name}'"),
                stmt_id,
            );
        }

        let FirType::Fun {
            args: param_types,
            ret,
        } = typ
        else {
            self.error(
                "FIR-F01",
                format!("function '{name}' has type that is not FirType::Fun"),
                stmt_id,
            );
            return;
        };

        let mut param_names: HashSet<String> = HashSet::new();
        let mut params_list: Vec<(String, FirType)> = Vec::with_capacity(args.len());
        for arg in args {
            if !param_names.insert(arg.name.clone()) {
                self.error(
                    "FIR-F04",
                    format!(
                        "function '{name}' has duplicate parameter name '{}'",
                        arg.name
                    ),
                    stmt_id,
                );
            }
            params_list.push((arg.name.clone(), arg.typ.clone()));
        }

        if name == "compute" {
            if **ret != FirType::Void {
                self.warn(
                    "FIR-F05",
                    format!("'compute' return type should be Void, got {ret:?}"),
                    stmt_id,
                );
            }
            if param_types.len() != 4 {
                self.warn(
                    "FIR-F06",
                    format!(
                        "'compute' should have 4 parameters \
                         (dsp*, count, inputs, outputs), got {}",
                        param_types.len()
                    ),
                    stmt_id,
                );
            }
        }

        let is_extern = body.is_none();
        if is_extern && warn_extern {
            self.warn(
                "FIR-F07",
                format!("function '{name}' has no body (prototype/extern declaration)"),
                stmt_id,
            );
        }

        self.symbols
            .functions
            .entry(name.to_string())
            .or_insert_with(|| FunctionSig {
                params: params_list,
                return_type: *ret.clone(),
                is_extern,
            });
    }

    // =========================================================================
    // Phase 2 — per-function scope analysis
    // =========================================================================

    /// Runs Phase 2/3 on every function body declared in the module.
    ///
    /// Only functions with a body are traversed. Prototype-only `DeclareFun`
    /// nodes contribute symbols during phase 1 but are not walked here.
    fn check_phase2(&mut self) {
        // Bail out if Phase 1 found a broken module skeleton.
        let FirMatch::Module { functions, .. } = match_fir(self.store, self.module_id) else {
            return;
        };
        let FirMatch::Block(stmts) = match_fir(self.store, functions) else {
            return;
        };

        for stmt_id in stmts {
            if let FirMatch::DeclareFun {
                name,
                typ,
                args,
                body: Some(body_id),
                ..
            } = match_fir(self.store, stmt_id)
            {
                let FirType::Fun { ret, .. } = typ else {
                    continue;
                };
                self.enter_function(
                    name,
                    *ret,
                    args.iter().map(|a| (a.name.clone(), a.typ.clone())),
                );
                self.check_stmt(body_id);
                self.leave_function();
            }
        }

        self.check_lifecycle_order();
    }

    /// **FIR-LC01** — detect struct fields read in `instanceConstants` that are
    /// only initialized in `instanceClear`.
    ///
    /// The standard DSP lifecycle is:
    /// 1. `instanceConstants(sample_rate)` — compute derived constants from SR
    /// 2. `instanceResetUserInterface()` — reset UI zones to defaults
    /// 3. `instanceClear()` — zero-initialize state arrays and counters
    /// 4. `compute(count, inputs, outputs)` — per-block DSP loop
    ///
    /// Any struct field that is **not** stored anywhere inside `instanceConstants`
    /// but **is** stored inside `instanceClear` will still hold its default
    /// zero-initialized value from C++ allocation when `instanceConstants` reads
    /// it.  In practice this means waveform index counters (e.g. `iWave48`)
    /// appear explicitly initialized only in `instanceClear`, yet a misplaced
    /// hoisting decision may cause `instanceConstants` to read them as if they
    /// had already been set — producing wrong constant values.
    ///
    /// The check emits FIR-LC01 for every `LoadVar(kStruct, name)` in
    /// `instanceConstants` where `name` ∈ (written-only-in-clear) set.
    fn check_lifecycle_order(&mut self) {
        // Locate instanceConstants and instanceClear function bodies.
        let FirMatch::Module { functions, .. } = match_fir(self.store, self.module_id) else {
            return;
        };
        let FirMatch::Block(stmts) = match_fir(self.store, functions) else {
            return;
        };

        let mut constants_body: Option<FirId> = None;
        let mut constants_fun_id: Option<FirId> = None;
        let mut clear_body: Option<FirId> = None;

        for stmt_id in &stmts {
            if let FirMatch::DeclareFun {
                name,
                body: Some(body_id),
                ..
            } = match_fir(self.store, *stmt_id)
            {
                match name.as_str() {
                    "instanceConstants" => {
                        constants_body = Some(body_id);
                        constants_fun_id = Some(*stmt_id);
                    }
                    "instanceClear" => {
                        clear_body = Some(body_id);
                    }
                    _ => {}
                }
            }
        }

        let (Some(constants_body), Some(constants_fun_id), Some(clear_body)) =
            (constants_body, constants_fun_id, clear_body)
        else {
            // One or both functions missing — other checks cover that.
            return;
        };

        // Fields written in instanceConstants (safely computed before any read).
        let constants_stores = collect_struct_stores(self.store, constants_body);
        // Fields written in instanceClear.
        let clear_stores = collect_struct_stores(self.store, clear_body);

        // Fields that instanceClear initializes but instanceConstants never writes
        // — reading them in instanceConstants yields an uninitialized value.
        let cleared_only: HashSet<&String> = clear_stores
            .iter()
            .filter(|n| !constants_stores.contains(*n))
            .collect();

        if cleared_only.is_empty() {
            return;
        }

        // Walk instanceConstants body for loads of those fields.
        let loads = collect_struct_loads(self.store, constants_body);
        for (field_name, load_id) in loads {
            if cleared_only.contains(&field_name) {
                self.error(
                    "FIR-LC01",
                    format!(
                        "struct field '{field_name}' is read in `instanceConstants` but is only \
                         initialized in `instanceClear` (which runs later); value is \
                         zero-initialized at this point"
                    ),
                    load_id,
                );
                // Also tag the diagnostic with the enclosing function name.
                if let Some(d) = self.diags.last_mut() {
                    d.context.function_name = Some("instanceConstants".to_owned());
                    d.context.variable_name = Some(field_name);
                }
            }
        }

        let _ = constants_fun_id; // available for future use
    }

    /// Initializes per-function verification state and seeds `kFunArgs`.
    fn enter_function(
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
    fn leave_function(&mut self) {
        self.scope_stack.pop();
        self.current_function = None;
        self.current_return_type = None;
        self.current_fun_args.clear();
    }

    // ── Scope resolution ──────────────────────────────────────────────────────

    /// Resolve a variable name+access to its declared `VarEntry`, if any.
    ///
    /// Returns `None` only when the variable is genuinely undeclared.
    ///
    /// `kStruct` accesses are validated against names declared in the
    /// `dsp_struct` block. The returned type remains a placeholder because
    /// checker phase 3 still relies on the explicit FIR node `typ` for struct
    /// accesses (name→type mapping is not tracked yet).
    fn resolve(&self, name: &str, access: AccessType) -> Option<VarEntry> {
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
    fn resolve_any_by_name(&self, name: &str) -> Option<VarEntry> {
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

    // ── Phase 3 helpers (type inference / compatibility) ────────────────────

    /// Returns the element type for pointer/array/vector containers.
    fn is_indexable_container_type(&self, typ: &FirType) -> Option<FirType> {
        match typ {
            FirType::Ptr(inner) => Some((**inner).clone()),
            FirType::Array(inner, _) => Some((**inner).clone()),
            FirType::Vector(inner, _) => Some((**inner).clone()),
            _ => None,
        }
    }

    /// Infers the semantic value type of one FIR value node.
    ///
    /// The checker prefers symbol table information when available, but falls
    /// back to the explicit type encoded on the FIR node to remain robust to
    /// partial symbol information (notably some `kStruct` accesses).
    fn infer_value_type(&self, id: FirId) -> Option<FirType> {
        match match_fir(self.store, id) {
            FirMatch::LoadVar { name, access, typ } => {
                // For `kStruct`, `resolve()` intentionally carries a placeholder
                // type because field names are unavailable at the type level.
                // Prefer the explicit FIR node type in that case.
                if access == AccessType::Struct {
                    Some(typ)
                } else {
                    self.resolve(&name, access).map(|e| e.typ).or(Some(typ))
                }
            }
            FirMatch::LoadTable {
                name, access, typ, ..
            } => {
                if access == AccessType::Struct {
                    Some(typ)
                } else {
                    self.resolve(&name, access)
                        .map(|e| {
                            if e.is_table {
                                e.typ
                            } else {
                                self.is_indexable_container_type(&e.typ).unwrap_or(e.typ)
                            }
                        })
                        .or(Some(typ))
                }
            }
            FirMatch::FunCall { name, typ, .. } => self
                .symbols
                .functions
                .get(&name)
                .map(|sig| sig.return_type.clone())
                .or(Some(typ)),
            _ => self.store.value_type(id),
        }
    }

    /// Returns `true` for integer scalar types accepted by index/condition rules.
    fn is_integer_type(&self, typ: &FirType) -> bool {
        matches!(typ, FirType::Int32 | FirType::Int64)
    }

    /// Returns `true` for scalar numeric-like types used by arithmetic checks.
    ///
    /// `Bool` is intentionally included because some FIR arithmetic/logical
    /// operations allow explicit bool/int mixtures.
    fn is_numeric_type(&self, typ: &FirType) -> bool {
        matches!(
            typ,
            FirType::Int32
                | FirType::Int64
                | FirType::Float32
                | FirType::Float64
                | FirType::FaustFloat
                | FirType::Quad
                | FirType::FixedPoint
                | FirType::Bool
        )
    }

    /// Returns `true` for floating-point-like scalar types.
    fn is_float_like_type(&self, typ: &FirType) -> bool {
        matches!(
            typ,
            FirType::Float32 | FirType::Float64 | FirType::FaustFloat | FirType::Quad
        )
    }

    /// Returns `true` when a type is accepted as an integer/boolean condition.
    fn is_int_or_bool_type(&self, typ: &FirType) -> bool {
        self.is_integer_type(typ) || *typ == FirType::Bool
    }

    /// Returns `true` when operands are identical or one of the allowed bool/int mixes.
    fn same_or_int_bool_mix(&self, lhs: &FirType, rhs: &FirType) -> bool {
        lhs == rhs
            || matches!(
                (lhs, rhs),
                (FirType::Bool, FirType::Int32)
                    | (FirType::Bool, FirType::Int64)
                    | (FirType::Int32, FirType::Bool)
                    | (FirType::Int64, FirType::Bool)
            )
    }

    /// Compatibility relation used for function-call argument warnings (`FC03`).
    ///
    /// This is intentionally broader than exact type equality and allows
    /// numeric-to-numeric calls, while binops remain stricter (`B01`).
    fn types_compatible(&self, actual: &FirType, expected: &FirType) -> bool {
        actual == expected || (self.is_numeric_type(actual) && self.is_numeric_type(expected))
    }

    /// Best-effort bit width lookup for bitcast validation.
    fn bit_width(&self, typ: &FirType) -> Option<u32> {
        match typ {
            FirType::Bool => Some(1),
            FirType::Int32 | FirType::Float32 => Some(32),
            FirType::Int64 | FirType::Float64 => Some(64),
            FirType::Quad => Some(128),
            FirType::Ptr(_) | FirType::Obj | FirType::Sound | FirType::UI | FirType::Meta => {
                Some(64)
            }
            _ => None,
        }
    }

    /// Computes a checker-side numeric promotion target for diagnostics.
    ///
    /// This does not rewrite FIR; it is used only to evaluate whether the
    /// declared result type of operations is plausible (`B03`).
    fn promoted_numeric_type(&self, lhs: &FirType, rhs: &FirType) -> Option<FirType> {
        if !self.is_numeric_type(lhs) || !self.is_numeric_type(rhs) {
            return None;
        }
        if lhs == rhs {
            return Some(lhs.clone());
        }
        if self.same_or_int_bool_mix(lhs, rhs) {
            return Some(
                if matches!(lhs, FirType::Int64) || matches!(rhs, FirType::Int64) {
                    FirType::Int64
                } else {
                    FirType::Int32
                },
            );
        }
        let rank = |t: &FirType| -> i32 {
            match t {
                FirType::Quad => 70,
                FirType::Float64 => 60,
                FirType::FaustFloat => 55,
                FirType::Float32 => 50,
                FirType::FixedPoint => 45,
                FirType::Int64 => 20,
                FirType::Int32 => 10,
                FirType::Bool => 0,
                _ => -1,
            }
        };
        let out = if rank(lhs) >= rank(rhs) { lhs } else { rhs };
        Some(out.clone())
    }

    /// Returns the expected result type for a binop given inferred operand types.
    fn expected_binop_result_type(
        &self,
        op: FirBinOp,
        lhs: &FirType,
        rhs: &FirType,
    ) -> Option<FirType> {
        match op {
            FirBinOp::Eq
            | FirBinOp::Ne
            | FirBinOp::Lt
            | FirBinOp::Le
            | FirBinOp::Gt
            | FirBinOp::Ge => Some(FirType::Int32),
            FirBinOp::And | FirBinOp::Or | FirBinOp::Xor => {
                if *lhs == FirType::Bool && *rhs == FirType::Bool {
                    Some(FirType::Bool)
                } else {
                    self.promoted_numeric_type(lhs, rhs)
                }
            }
            _ => self.promoted_numeric_type(lhs, rhs),
        }
    }

    /// Detects literal zero values for division-by-zero diagnostics.
    fn const_is_zero(&self, id: FirId) -> bool {
        match match_fir(self.store, id) {
            FirMatch::Int32 { value, .. } => value == 0,
            FirMatch::Int64 { value, .. } => value == 0,
            FirMatch::Float32 { value, .. } => value == 0.0,
            FirMatch::Float64 { value, .. } => value == 0.0,
            FirMatch::Bool { value, .. } => !value,
            _ => false,
        }
    }

    /// Shared condition-type check for `If`/`Select2`/`WhileLoop`.
    ///
    /// `code` and `what` parameterize the emitted diagnostic.
    fn check_int_or_bool_condition(
        &mut self,
        id: FirId,
        cond: FirId,
        code: &'static str,
        what: &str,
    ) {
        if let Some(cond_ty) = self.infer_value_type(cond)
            && !self.is_int_or_bool_type(&cond_ty)
        {
            self.error(
                code,
                format!("{what} condition should be Int32, Int64, or Bool, got {cond_ty:?}"),
                id,
            );
        }
    }

    /// Specialized condition-type check for `Switch` (integers only).
    fn check_switch_condition_type(&mut self, id: FirId, cond: FirId) {
        if let Some(cond_ty) = self.infer_value_type(cond)
            && !self.is_integer_type(&cond_ty)
        {
            self.error(
                "FIR-SW01",
                format!("Switch condition should be Int32 or Int64, got {cond_ty:?}"),
                id,
            );
        }
    }

    /// Rejects `Void`-typed expressions in positions that require a real value.
    fn check_required_value(&mut self, id: FirId, value: FirId, what: &str) {
        if matches!(self.infer_value_type(value), Some(FirType::Void)) {
            self.error(
                "FIR-V01",
                format!("{what} must produce a non-Void value"),
                id,
            );
        }
    }

    /// Validates operand/result typing for one `BinOp` node.
    fn check_binop_types(
        &mut self,
        id: FirId,
        op: FirBinOp,
        lhs: FirId,
        rhs: FirId,
        declared: &FirType,
    ) {
        let lhs_ty = self.infer_value_type(lhs);
        let rhs_ty = self.infer_value_type(rhs);
        let (Some(lhs_ty), Some(rhs_ty)) = (lhs_ty, rhs_ty) else {
            return;
        };

        if !self.same_or_int_bool_mix(&lhs_ty, &rhs_ty) {
            self.error(
                "FIR-B01",
                format!("BinOp operands have incompatible types: {lhs_ty:?} vs {rhs_ty:?}"),
                id,
            );
        }
        if !self.is_numeric_type(&lhs_ty) || !self.is_numeric_type(&rhs_ty) {
            self.error(
                "FIR-B02",
                format!("BinOp operands must be numeric, got {lhs_ty:?} and {rhs_ty:?}"),
                id,
            );
        }
        if let Some(expected) = self.expected_binop_result_type(op, &lhs_ty, &rhs_ty)
            && &expected != declared
        {
            self.warn(
                "FIR-B03",
                format!(
                    "BinOp declared result type {declared:?} is inconsistent with operands \
                     ({lhs_ty:?}, {rhs_ty:?}); expected {expected:?}"
                ),
                id,
            );
        }
        if op == FirBinOp::Div && self.const_is_zero(rhs) {
            self.warn("FIR-B04", "division by constant zero in BinOp", id);
        }
    }

    /// Validates unary negation operand typing.
    fn check_neg_type(&mut self, id: FirId, value: FirId) {
        if let Some(val_ty) = self.infer_value_type(value)
            && !self.is_numeric_type(&val_ty)
        {
            self.error(
                "FIR-U01",
                format!("Neg operand must be numeric, got {val_ty:?}"),
                id,
            );
        }
    }

    /// Validates numeric cast usage and emits no-op cast warnings.
    fn check_cast_type(&mut self, id: FirId, target: &FirType, value: FirId) {
        if let Some(src_ty) = self.infer_value_type(value) {
            if &src_ty == target {
                self.warn("FIR-U02", format!("Cast is a no-op to {target:?}"), id);
            }
            if !self.is_numeric_type(&src_ty) || !self.is_numeric_type(target) {
                self.error(
                    "FIR-U03",
                    format!(
                        "Cast requires numeric source/target types, got {src_ty:?} -> {target:?}"
                    ),
                    id,
                );
            }
        }
    }

    /// Validates bitcast width compatibility (warning-only on mismatch).
    fn check_bitcast_type(&mut self, id: FirId, target: &FirType, value: FirId) {
        if let Some(src_ty) = self.infer_value_type(value) {
            let src_w = self.bit_width(&src_ty);
            let dst_w = self.bit_width(target);
            if let (Some(sw), Some(dw)) = (src_w, dst_w)
                && sw != dw
            {
                self.warn(
                    "FIR-U04",
                    format!("Bitcast width mismatch: {src_ty:?} ({sw}) -> {target:?} ({dw})"),
                    id,
                );
            }
        }
    }

    /// Validates `Select2` condition/branch/result typing.
    fn check_select2_types(
        &mut self,
        id: FirId,
        cond: FirId,
        then_value: FirId,
        else_value: FirId,
        declared: &FirType,
    ) {
        self.check_int_or_bool_condition(id, cond, "FIR-C01", "Select2");
        let then_ty = self.infer_value_type(then_value);
        let else_ty = self.infer_value_type(else_value);
        if let (Some(tt), Some(et)) = (then_ty, else_ty) {
            if tt != et {
                self.warn(
                    "FIR-C02",
                    format!("Select2 branches have different types: {tt:?} vs {et:?}"),
                    id,
                );
            }
            if &tt != declared && &et != declared {
                self.warn(
                    "FIR-C03",
                    format!(
                        "Select2 declared result type {declared:?} does not match branch types \
                         ({tt:?}, {et:?})"
                    ),
                    id,
                );
            }
        }
    }

    /// Validates function call arity/signature/result typing and math-call conventions.
    fn check_fun_call_types(&mut self, id: FirId, name: &str, args: &[FirId], declared: &FirType) {
        if let Some(sig) = self.symbols.functions.get(name).cloned() {
            if sig.params.len() != args.len() {
                self.error(
                    "FIR-FC02",
                    format!(
                        "call to '{name}' has {} args, expected {}",
                        args.len(),
                        sig.params.len()
                    ),
                    id,
                );
            }
            for (i, (arg_id, (_pname, pty))) in args.iter().zip(sig.params.iter()).enumerate() {
                if let Some(actual_ty) = self.infer_value_type(*arg_id)
                    && !self.types_compatible(&actual_ty, pty)
                {
                    self.warn(
                        "FIR-FC03",
                        format!(
                            "call to '{name}' arg #{i} has type {actual_ty:?}, expected {pty:?}"
                        ),
                        id,
                    );
                }
            }
            if &sig.return_type != declared {
                self.warn(
                    "FIR-FC04",
                    format!(
                        "call to '{name}' declared result type {declared:?} differs from function \
                         signature return type {:?}",
                        sig.return_type
                    ),
                    id,
                );
            }
        } else {
            self.error(
                "FIR-FC01",
                format!("call to undeclared function '{name}'"),
                id,
            );
        }

        self.check_math_call(id, name, args);
    }

    /// Applies math-specific naming/arity/argument diagnostics (`MA*`) to a call.
    ///
    /// This runs in addition to generic function-call checks and accepts both
    /// canonical and `std::`-prefixed symbols.
    fn check_math_call(&mut self, id: FirId, name: &str, args: &[FirId]) {
        let raw = name.strip_prefix("std::").unwrap_or(name);
        let Some(op) = FirMathOp::from_symbol(name) else {
            if raw == "abs"
                && let Some(arg) = args.first().and_then(|arg| self.infer_value_type(*arg))
                && self.is_float_like_type(&arg)
            {
                self.warn(
                    "FIR-MA04",
                    "use 'fabs' for floating-point absolute value (got 'abs')",
                    id,
                );
            }
            return;
        };

        let expected_arity = match op {
            FirMathOp::Pow
            | FirMathOp::Min
            | FirMathOp::Max
            | FirMathOp::Atan2
            | FirMathOp::Fmod
            | FirMathOp::Remainder => 2,
            _ => 1,
        };
        match expected_arity {
            1 if args.len() != 1 => self.warn(
                "FIR-MA01",
                format!(
                    "math op '{}' expects 1 arg, got {}",
                    op.symbol(),
                    args.len()
                ),
                id,
            ),
            2 if args.len() != 2 => self.warn(
                "FIR-MA02",
                format!(
                    "math op '{}' expects 2 args, got {}",
                    op.symbol(),
                    args.len()
                ),
                id,
            ),
            _ => {}
        }

        let is_float_math = !matches!(op, FirMathOp::Abs);
        if is_float_math && expected_arity == args.len() {
            for (i, arg_id) in args.iter().enumerate() {
                if let Some(arg_ty) = self.infer_value_type(*arg_id)
                    && (self.is_integer_type(&arg_ty) || arg_ty == FirType::Bool)
                {
                    self.warn(
                        "FIR-MA03",
                        format!(
                            "math op '{}' arg #{i} is integer-like ({arg_ty:?}); \
                             floating-point argument expected",
                            op.symbol()
                        ),
                        id,
                    );
                }
            }
        }

        if raw == "fabs"
            && let Some(arg_ty) = args.first().and_then(|arg| self.infer_value_type(*arg))
            && (self.is_integer_type(&arg_ty) || arg_ty == FirType::Bool)
        {
            self.warn(
                "FIR-MA04",
                format!("'fabs' called with integer-like argument {arg_ty:?}"),
                id,
            );
        }
    }

    /// Resolves one table access to its declared element type.
    ///
    /// FIR-T04 (error): the name has no declaration at all - not in the scope
    /// stack, not a global, and not a DSP-struct field. An access to a name no
    /// declaration backs cannot compile in any backend, so silence here lets an
    /// invalid module through behind a green report. FIR-T03 (warning): the
    /// name is declared but not as a table or indexable container.
    fn table_access_elem_type(
        &mut self,
        id: FirId,
        name: &str,
        access: AccessType,
        what: &str,
    ) -> Option<FirType> {
        let declared = match access {
            AccessType::Struct => self
                .symbols
                .struct_field_types
                .get(name)
                .cloned()
                .map(|typ| (typ, false)),
            _ => self
                .resolve(name, access)
                .map(|entry| (entry.typ, entry.is_table)),
        };
        let Some((typ, is_table)) = declared else {
            self.error("FIR-T04", format!("{what} '{name}' has no declaration"), id);
            return None;
        };
        let elem = if is_table {
            Some(typ.clone())
        } else {
            self.is_indexable_container_type(&typ)
        };
        if elem.is_none() {
            self.warn(
                "FIR-T03",
                format!("{what} '{name}' refers to a non-table declaration"),
                id,
            );
        }
        elem
    }

    /// Validates `LoadTable` index typing and declaration/table consistency.
    fn check_load_table_types(
        &mut self,
        id: FirId,
        name: &str,
        access: AccessType,
        index: FirId,
        declared_elem_type: &FirType,
    ) {
        if let Some(index_ty) = self.infer_value_type(index)
            && !self.is_integer_type(&index_ty)
        {
            self.error(
                "FIR-T01",
                format!("table index must be Int32 or Int64, got {index_ty:?}"),
                id,
            );
        }
        if let Some(expected_elem_type) = self.table_access_elem_type(id, name, access, "LoadTable")
            && expected_elem_type != *declared_elem_type
        {
            self.warn(
                "FIR-T03",
                format!(
                    "LoadTable '{name}' element type {declared_elem_type:?} differs from \
                     declaration {:?}",
                    expected_elem_type
                ),
                id,
            );
        }
    }

    /// Validates `StoreTable` index typing and stored element type compatibility.
    fn check_store_table_types(
        &mut self,
        id: FirId,
        name: &str,
        access: AccessType,
        index: FirId,
        value: FirId,
    ) {
        if let Some(index_ty) = self.infer_value_type(index)
            && !self.is_integer_type(&index_ty)
        {
            self.error(
                "FIR-T01",
                format!("table index must be Int32 or Int64, got {index_ty:?}"),
                id,
            );
        }
        if let Some(expected_elem_type) =
            self.table_access_elem_type(id, name, access, "StoreTable")
            && let Some(val_ty) = self.infer_value_type(value)
            && val_ty != expected_elem_type
        {
            self.error(
                "FIR-T02",
                format!(
                    "StoreTable value type {val_ty:?} does not match element type {:?}",
                    expected_elem_type
                ),
                id,
            );
        }
    }

    /// Validates that one `soundfile` slot name resolves to a DSP struct field
    /// of type [`FirType::Sound`].
    fn check_soundfile_slot(&mut self, id: FirId, var: &str) {
        match self.symbols.struct_field_types.get(var) {
            Some(FirType::Sound) => {}
            Some(found) => self.warn(
                "FIR-SF01",
                format!(
                    "soundfile access '{var}' refers to struct field of type {found:?}, expected Sound"
                ),
                id,
            ),
            None => self.warn(
                "FIR-SC09",
                format!("kStruct variable '{var}' is not declared in dsp_struct"),
                id,
            ),
        }
    }

    /// Validates one soundfile subscript-like operand (`part`, `chan`, `idx`).
    fn check_soundfile_index_like(&mut self, id: FirId, value: FirId, what: &str) {
        self.check_value(value);
        if let Some(index_ty) = self.infer_value_type(value)
            && !self.is_integer_type(&index_ty)
        {
            self.error(
                "FIR-T01",
                format!("soundfile {what} must be Int32 or Int64, got {index_ty:?}"),
                id,
            );
        }
    }

    /// Warns when a `Drop` discards a non-void function return value.
    fn check_fun_call_drop_use(&mut self, id: FirId, value: FirId) {
        if let FirMatch::FunCall { name, typ, .. } = match_fir(self.store, value)
            && typ != FirType::Void
        {
            self.warn(
                "FIR-FC04",
                format!("discarded non-void return value from '{name}' ({typ:?})"),
                id,
            );
        }
    }

    // ── Statement traversal ───────────────────────────────────────────────────

    /// Traverses one statement node and dispatches statement-level checks.
    ///
    /// This method is the main recursive entry point for Phase 2/3 body walks.
    fn check_stmt(&mut self, id: FirId) {
        match match_fir(self.store, id) {
            FirMatch::Block(stmts) => self.check_block(stmts),
            FirMatch::DeclareVar {
                name,
                typ,
                access,
                init,
            } => {
                self.check_declare_var(id, name, typ, access, init);
            }
            FirMatch::DeclareTable {
                name,
                access,
                elem_type,
                values,
            } => {
                for v in values {
                    self.check_value(v);
                }
                self.scope_stack.declare_table(name, elem_type, access);
            }
            FirMatch::StoreVar {
                name,
                access,
                value,
            } => {
                self.check_value(value);
                self.check_required_value(id, value, "StoreVar value");
                self.check_store_var(id, &name, access);
            }
            FirMatch::StoreTable {
                name,
                access,
                index,
                value,
            } => {
                self.check_value(index);
                self.check_value(value);
                self.check_store_var(id, &name, access);
                self.check_store_table_types(id, &name, access, index, value);
            }
            FirMatch::ShiftArrayVar { name, access, .. } => {
                // ShiftArrayVar modifies an array variable in-place; treat as a store.
                self.check_store_var(id, &name, access);
            }
            FirMatch::Drop(val) => {
                self.check_value(val);
                self.check_fun_call_drop_use(id, val);
            }
            FirMatch::Return(val) => self.check_return(id, val),
            FirMatch::If {
                cond,
                then_block,
                else_block,
            } => {
                self.check_value(cond);
                self.check_int_or_bool_condition(id, cond, "FIR-C04", "If");
                self.check_if(then_block, else_block);
            }
            FirMatch::ForLoop {
                var,
                init,
                end,
                step,
                body,
                ..
            } => {
                self.check_for_loop(id, &var, init, end, step, body);
            }
            FirMatch::SimpleForLoop {
                var, upper, body, ..
            } => {
                self.check_simple_for_loop(id, &var, upper, body);
            }
            FirMatch::IteratorForLoop {
                iterators, body, ..
            } => {
                self.check_iterator_for_loop(body, &iterators);
            }
            FirMatch::WhileLoop { cond, body } => {
                self.check_value(cond);
                self.check_int_or_bool_condition(id, cond, "FIR-L03", "WhileLoop");
                self.scope_stack.push(FrameKind::Block);
                self.check_stmt(body);
                self.scope_stack.pop();
            }
            FirMatch::Switch {
                cond,
                cases,
                default,
            } => {
                self.check_value(cond);
                self.check_switch_condition_type(id, cond);
                self.check_switch(id, cases, default);
            }
            FirMatch::Control { cond, stmt } => {
                self.check_value(cond);
                self.check_stmt(stmt);
            }
            FirMatch::AddSoundfile { var, .. } => {
                self.check_soundfile_slot(id, &var);
            }
            // UI, meta, null — no scope-relevant content
            _ => {}
        }
    }

    // ── Block ─────────────────────────────────────────────────────────────────

    /// Verifies a lexical `Block` with a fresh frame and return-flow tracking.
    fn check_block(&mut self, stmts: Vec<FirId>) {
        self.scope_stack.push(FrameKind::Block);
        let mut returned = false;
        for (i, stmt_id) in stmts.iter().enumerate() {
            if returned {
                // R03: dead code after Return
                self.warn("FIR-R03", "unreachable statement after Return", *stmt_id);
                break;
            }
            // Detect a Return statement to set the `returned` flag for the next iteration.
            if matches!(match_fir(self.store, *stmt_id), FirMatch::Return(_)) {
                returned = true;
                // Still check the Return itself.
                let _ = i;
            }
            self.check_stmt(*stmt_id);
        }
        self.scope_stack.pop();
    }

    // ── DeclareVar ────────────────────────────────────────────────────────────

    /// Registers and validates a local `DeclareVar` inside a function body.
    fn check_declare_var(
        &mut self,
        id: FirId,
        name: String,
        typ: FirType,
        access: AccessType,
        init: Option<FirId>,
    ) {
        // SC07: kFunArgs must not be re-declared inside a function body
        if access == AccessType::FunArgs {
            self.error(
                "FIR-SC07",
                format!("kFunArgs variable '{name}' re-declared inside function body"),
                id,
            );
        }
        if !matches!(
            access,
            AccessType::Stack | AccessType::Loop | AccessType::FunArgs
        ) {
            self.error(
                "FIR-SC10",
                format!(
                    "local DeclareVar '{name}' uses non-local access type {access:?} \
                     (expected Stack or Loop)"
                ),
                id,
            );
        }

        if let Some(init_id) = init {
            self.check_value(init_id);
            self.check_required_value(id, init_id, "DeclareVar initializer");
        }

        let init_status = if init.is_some() {
            InitStatus::Yes
        } else {
            InitStatus::No
        };
        self.scope_stack.declare(name, typ, access, init_status);
    }

    // ── LoadVar (value) ───────────────────────────────────────────────────────

    /// Validates a variable load (`LoadVar` / `LoadVarAddress`) against scope state.
    fn check_load_var(&mut self, id: FirId, name: &str, access: AccessType) {
        if access == AccessType::Struct && !self.symbols.struct_field_names.contains(name) {
            self.warn(
                "FIR-SC09",
                format!("kStruct variable '{name}' is not declared in dsp_struct"),
                id,
            );
            return;
        }
        match self.resolve(name, access) {
            None => {
                if let Some(entry) = self.resolve_any_by_name(name) {
                    self.error(
                        "FIR-SC02",
                        format!(
                            "variable '{name}' accessed as {access:?} \
                             but declared as {:?}",
                            entry.access
                        ),
                        id,
                    );
                } else {
                    // SC01: variable not declared
                    self.error(
                        "FIR-SC01",
                        format!("use of undeclared variable '{name}'"),
                        id,
                    );
                }
            }
            Some(entry) => {
                // SC02: access type must match declaration
                if entry.access != access {
                    self.error(
                        "FIR-SC02",
                        format!(
                            "variable '{name}' accessed as {access:?} \
                             but declared as {:?}",
                            entry.access
                        ),
                        id,
                    );
                }
                // SC03: warn if kStack variable is uninitialized
                if access == AccessType::Stack && entry.init == InitStatus::No {
                    self.warn(
                        "FIR-SC03",
                        format!("variable '{name}' may be used before initialization"),
                        id,
                    );
                }
            }
        }
    }

    // ── StoreVar (statement) ──────────────────────────────────────────────────

    /// Validates a variable store target and updates initialization state.
    fn check_store_var(&mut self, id: FirId, name: &str, access: AccessType) {
        if access == AccessType::Struct && !self.symbols.struct_field_names.contains(name) {
            self.warn(
                "FIR-SC09",
                format!("kStruct variable '{name}' is not declared in dsp_struct"),
                id,
            );
            return;
        }
        match self.resolve(name, access) {
            None => {
                if let Some(entry) = self.resolve_any_by_name(name) {
                    self.error(
                        "FIR-SC05",
                        format!(
                            "variable '{name}' stored as {access:?} \
                             but declared as {:?}",
                            entry.access
                        ),
                        id,
                    );
                } else {
                    self.error(
                        "FIR-SC04",
                        format!("store to undeclared variable '{name}'"),
                        id,
                    );
                }
            }
            Some(entry) => {
                // SC05: access type must match declaration
                if entry.access != access {
                    self.error(
                        "FIR-SC05",
                        format!(
                            "variable '{name}' stored as {access:?} \
                             but declared as {:?}",
                            entry.access
                        ),
                        id,
                    );
                }
                // Mark as initialized (for kStack / kLoop vars in the scope stack)
                if matches!(access, AccessType::Stack | AccessType::Loop) {
                    self.scope_stack.mark_initialized(name);
                }
            }
        }
    }

    // ── ForLoop ───────────────────────────────────────────────────────────────

    /// Validates a full `ForLoop` statement and checks its body in a loop frame.
    fn check_for_loop(
        &mut self,
        id: FirId,
        var: &str,
        init: FirId,
        end: FirId,
        step: FirId,
        body: FirId,
    ) {
        // L01 / L02: the init node should be DeclareVar(kLoop)
        match match_fir(self.store, init) {
            FirMatch::DeclareVar {
                name: ref decl_name,
                ref typ,
                access,
                ..
            } if decl_name == var => {
                if access != AccessType::Loop {
                    self.error(
                        "FIR-L01",
                        format!(
                            "ForLoop variable '{var}' init is not a kLoop DeclareVar \
                             (got {access:?})"
                        ),
                        init,
                    );
                }
                if !matches!(typ, FirType::Int32 | FirType::Int64) {
                    self.error(
                        "FIR-L02",
                        format!(
                            "ForLoop variable '{var}' type should be Int32 or Int64, \
                             got {typ:?}"
                        ),
                        init,
                    );
                }
            }
            _ => {
                // init is not a DeclareVar for the expected loop variable
                self.error(
                    "FIR-L01",
                    format!("ForLoop '{var}' init is not a DeclareVar for the loop variable"),
                    init,
                );
            }
        }

        // Push a Loop frame containing the loop variable
        self.scope_stack.push(FrameKind::Loop);

        // Process the init statement (registers the loop variable in the loop frame)
        self.check_stmt(init);

        // Traverse end condition and step
        self.check_value(end);
        self.check_stmt(step);

        // L04: body must be non-empty
        if let FirMatch::Block(ref stmts) = match_fir(self.store, body)
            && stmts.is_empty()
        {
            self.warn("FIR-L04", format!("ForLoop '{var}' body is empty"), id);
        }
        self.check_stmt(body);

        self.scope_stack.pop();
    }

    // ── SimpleForLoop ─────────────────────────────────────────────────────────

    /// Validates a `SimpleForLoop` and introduces its implicit loop variable.
    fn check_simple_for_loop(&mut self, id: FirId, var: &str, upper: FirId, body: FirId) {
        self.scope_stack.push(FrameKind::Loop);

        // Implicit loop variable: kLoop, Int32, initialized
        self.scope_stack.declare(
            var.to_string(),
            FirType::Int32,
            AccessType::Loop,
            InitStatus::Yes,
        );

        self.check_value(upper);

        // L04: body must be non-empty
        if let FirMatch::Block(ref stmts) = match_fir(self.store, body)
            && stmts.is_empty()
        {
            self.warn(
                "FIR-L04",
                format!("SimpleForLoop '{var}' body is empty"),
                id,
            );
        }
        self.check_stmt(body);

        self.scope_stack.pop();
    }

    // ── IteratorForLoop ───────────────────────────────────────────────────────

    /// Validates an `IteratorForLoop` by predeclaring all iterator names as loop vars.
    fn check_iterator_for_loop(&mut self, body: FirId, iterators: &[String]) {
        self.scope_stack.push(FrameKind::Loop);
        for iter in iterators {
            self.scope_stack.declare(
                iter.clone(),
                FirType::Int32,
                AccessType::Loop,
                InitStatus::Yes,
            );
        }
        self.check_stmt(body);
        self.scope_stack.pop();
    }

    // ── If ────────────────────────────────────────────────────────────────────

    /// Verifies both branches of an `If` and merges variable init states.
    ///
    /// Declarations remain branch-local; only initialization information for
    /// pre-existing variables is merged back into the outer frame.
    fn check_if(&mut self, then_block: FirId, else_block: Option<FirId>) {
        let pre = self.scope_stack.snapshot_inits();

        // Traverse then branch
        self.scope_stack.push(FrameKind::Block);
        self.check_stmt(then_block);
        self.scope_stack.pop();
        let then_changes = self.scope_stack.diff_inits(&pre);
        self.scope_stack.restore_inits(&pre);

        // Traverse else branch
        let else_changes = if let Some(else_id) = else_block {
            self.scope_stack.push(FrameKind::Block);
            self.check_stmt(else_id);
            self.scope_stack.pop();
            let changes = self.scope_stack.diff_inits(&pre);
            self.scope_stack.restore_inits(&pre);
            changes
        } else {
            Vec::new()
        };

        // Merge: both branches initialized the var → Yes; only one → Maybe
        let all: HashSet<(usize, String)> = then_changes
            .iter()
            .chain(else_changes.iter())
            .cloned()
            .collect();
        for (fi, name) in all {
            let in_then = then_changes.contains(&(fi, name.clone()));
            let in_else = else_changes.contains(&(fi, name.clone()));
            let status = if in_then && in_else {
                InitStatus::Yes
            } else {
                InitStatus::Maybe
            };
            self.scope_stack.set_init(fi, &name, status);
        }
    }

    // ── Return ────────────────────────────────────────────────────────────────

    /// Validates return statements against the current function return type.
    fn check_return(&mut self, id: FirId, value: Option<FirId>) {
        if let Some(val_id) = value {
            self.check_value(val_id);
            self.check_required_value(id, val_id, "Return expression");
            if let Some(ret_ty) = &self.current_return_type
                && let Some(val_ty) = self.infer_value_type(val_id)
                && val_ty != *ret_ty
            {
                self.error(
                    "FIR-R01",
                    format!(
                        "Return value type {val_ty:?} does not match function return type {ret_ty:?}"
                    ),
                    id,
                );
            }
        } else {
            // R02: Return(None) in a non-Void function
            if let Some(ret_ty) = &self.current_return_type
                && *ret_ty != FirType::Void
            {
                self.warn(
                    "FIR-R02",
                    format!(
                        "Return without value in function '{}' whose return type is {:?}",
                        self.current_function.as_deref().unwrap_or("?"),
                        ret_ty
                    ),
                    id,
                );
            }
        }
        // R03 is handled by check_block which detects statements after a Return.
    }

    // ── Switch ────────────────────────────────────────────────────────────────

    /// Validates `Switch` case structure and traverses all branch bodies.
    fn check_switch(&mut self, id: FirId, cases: Vec<(i64, FirId)>, default: Option<FirId>) {
        // SW03: at least one case
        if cases.is_empty() {
            self.warn("FIR-SW03", "Switch has no cases", id);
        }

        // SW02: no duplicate case values
        let mut seen_vals: HashSet<i64> = HashSet::new();
        for &(val, case_body) in &cases {
            if !seen_vals.insert(val) {
                self.error(
                    "FIR-SW02",
                    format!("Switch has duplicate case value {val}"),
                    id,
                );
            }
            self.scope_stack.push(FrameKind::Block);
            self.check_stmt(case_body);
            self.scope_stack.pop();
        }

        if let Some(default_body) = default {
            self.scope_stack.push(FrameKind::Block);
            self.check_stmt(default_body);
            self.scope_stack.pop();
        }
    }

    // ── Value traversal ───────────────────────────────────────────────────────

    /// Traverses one value expression and dispatches value-level checks.
    ///
    /// This recursively descends into expression children before applying local
    /// typing/scope checks for the current value node.
    fn check_value(&mut self, id: FirId) {
        match match_fir(self.store, id) {
            FirMatch::LoadVar { name, access, .. } => {
                self.check_load_var(id, &name, access);
            }
            FirMatch::LoadVarAddress { name, access, .. } => {
                self.check_load_var(id, &name, access);
            }
            FirMatch::TeeVar {
                name,
                access,
                value,
                ..
            } => {
                // TeeVar = store + load: check that the target is declared
                self.check_value(value);
                self.check_required_value(id, value, "TeeVar value");
                self.check_store_var(id, &name, access);
            }
            FirMatch::LoadTable {
                name,
                access,
                index,
                typ,
            } => {
                self.check_load_var(id, &name, access);
                self.check_value(index);
                self.check_load_table_types(id, &name, access, index, &typ);
            }
            FirMatch::BinOp { op, lhs, rhs, typ } => {
                self.check_value(lhs);
                self.check_value(rhs);
                self.check_binop_types(id, op, lhs, rhs, &typ);
            }
            FirMatch::Neg { value, .. } => {
                self.check_value(value);
                self.check_neg_type(id, value);
            }
            FirMatch::Cast { typ, value } => {
                self.check_value(value);
                self.check_cast_type(id, &typ, value);
            }
            FirMatch::Bitcast { typ, value } => {
                self.check_value(value);
                self.check_bitcast_type(id, &typ, value);
            }
            FirMatch::Select2 {
                cond,
                then_value,
                else_value,
                typ,
            } => {
                self.check_value(cond);
                self.check_value(then_value);
                self.check_value(else_value);
                self.check_select2_types(id, cond, then_value, else_value, &typ);
            }
            FirMatch::FunCall { name, args, typ } => {
                for &arg in &args {
                    self.check_value(arg);
                }
                self.check_fun_call_types(id, &name, &args, &typ);
            }
            FirMatch::ValueArray { values, .. } => {
                for (index, v) in values.into_iter().enumerate() {
                    self.check_value(v);
                    self.check_required_value(id, v, &format!("ValueArray element #{index}"));
                }
            }
            FirMatch::LoadSoundfileLength { var, part }
            | FirMatch::LoadSoundfileRate { var, part } => {
                self.check_soundfile_slot(id, &var);
                self.check_soundfile_index_like(id, part, "part");
            }
            FirMatch::LoadSoundfileBuffer {
                var,
                chan,
                part,
                idx,
                ..
            } => {
                self.check_soundfile_slot(id, &var);
                self.check_soundfile_index_like(id, chan, "channel");
                self.check_soundfile_index_like(id, part, "part");
                self.check_soundfile_index_like(id, idx, "index");
            }
            // Leaf value nodes (literals, NullValue, NewDsp, etc.) — nothing to check
            _ => {}
        }
    }
}

/// Parses `inputN` aliases used in `compute` into a zero-based index.
fn input_alias_index(name: &str) -> Option<usize> {
    name.strip_prefix("input")?.parse::<usize>().ok()
}

/// Parses `outputN` aliases used in `compute` into a zero-based index.
fn output_alias_index(name: &str) -> Option<usize> {
    name.strip_prefix("output")?.parse::<usize>().ok()
}

// ─── Lifecycle helpers (FIR-LC01) ─────────────────────────────────────────────

/// Iteratively collects all struct field names that appear as **store targets**
/// anywhere in the FIR subtree rooted at `root`.
///
/// This covers `StoreVar(kStruct)` and `TeeVar(kStruct)` (which both write a
/// struct field).  `StoreTable(kStruct)` is excluded because table elements are
/// always zero-initialized by `DeclareTable` and are not in scope for the
/// lifecycle uninitialized-read check.
fn collect_struct_stores(store: &FirStore, root: FirId) -> HashSet<String> {
    let mut names: HashSet<String> = HashSet::new();
    let mut worklist = vec![root];
    while let Some(id) = worklist.pop() {
        let node = match_fir(store, id);
        match &node {
            FirMatch::StoreVar {
                access: AccessType::Struct,
                name,
                ..
            } => {
                names.insert(name.clone());
            }
            FirMatch::TeeVar {
                access: AccessType::Struct,
                name,
                ..
            } => {
                names.insert(name.clone());
            }
            _ => {}
        }
        worklist.extend(child_ids(&node));
    }
    names
}

/// Iteratively collects all `(field_name, load_node_id)` pairs for
/// `LoadVar(kStruct)` reads found anywhere in the FIR subtree rooted at `root`.
fn collect_struct_loads(store: &FirStore, root: FirId) -> Vec<(String, FirId)> {
    let mut result: Vec<(String, FirId)> = Vec::new();
    let mut worklist = vec![root];
    while let Some(id) = worklist.pop() {
        let node = match_fir(store, id);
        if let FirMatch::LoadVar {
            access: AccessType::Struct,
            name,
            ..
        } = &node
        {
            result.push((name.clone(), id));
        }
        worklist.extend(child_ids(&node));
    }
    result
}

/// Returns a non-negative constant index from a `kFunArgs` table access node.
///
/// Only `Int32` literals with `value >= 0` are accepted.
fn funargs_constant_index(store: &FirStore, id: FirId) -> Option<usize> {
    match match_fir(store, id) {
        FirMatch::Int32 { value, .. } if value >= 0 => usize::try_from(value).ok(),
        _ => None,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
