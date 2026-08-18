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
//! | FIR-F08 | W | `frame` is declared but the canonical `compute` body is not empty |
//! | FIR-F09 | W | `control`/`frame` body references the block `count` argument |
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
//! | FIR-D01  | W | `Drop` discards a side-effect-free value |
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

use crate::inliner::is_obviously_side_effect_free_value;
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
    /// Body statement id, when the function has one.
    pub body: Option<FirId>,
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
    ctx.check_table_fill_coverage();
    ctx.check_fill_call_extent();
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
    /// Sub-module names already seen, across the whole nesting tree (FIR-T04).
    sub_module_names: HashSet<String>,
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

mod context;
mod module_shape;
mod phases;
mod statements;
mod table_fill;
mod types;
mod values;

#[cfg(test)]
mod tests;
