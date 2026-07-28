# FIR Module Verifier — Implementation Plan

**Date:** 2026-02-23
**Status:** Draft
**Branch target:** `fir-module-verifier`
**Crate:** `crates/fir` (new module `src/checker.rs`)

---

## Table of Contents

1. [Motivation and Context](#1-motivation-and-context)
2. [Analysis of the C++ Checkers](#2-analysis-of-the-c-checkers)
3. [Semantic Verification of an IR — Theory](#3-semantic-verification-of-an-ir--theory)
4. [FIR Module Structure Reminder](#4-fir-module-structure-reminder)
5. [Complete Inventory of Checks](#5-complete-inventory-of-checks)
6. [Rust Architecture](#6-rust-architecture)
7. [Implementation Phases](#7-implementation-phases)
8. [Integration in the Pipeline](#8-integration-in-the-pipeline)
9. [Test Strategy](#9-test-strategy)
10. [Relationship with `diagnostics` Crate](#10-relationship-with-diagnostics-crate)

---

## 1. Motivation and Context

The C++ Faust compiler contains two partial checkers applied late in the pipeline:

| C++ class | File | Scope |
|---|---|---|
| `FIRTypeChecker` | `compiler/generator/fir_to_fir.hh` | Warns on suspicious casts and type mismatches in binops |
| `FIRCodeChecker` | `compiler/generator/fir/fir_code_checker.hh` | Checks variable scopes and function arities within a block |
| `FIRVarChecker` | `compiler/generator/fir_to_fir.hh` | Checks scope of named address accesses |

These checkers are **partial**: they operate on isolated blocks or function bodies, without any cross-function context or awareness of the full module structure (struct declarations, global variables, function registry).

In the Rust port, the FIR is the last stable IR before code generation. **A robust verifier at this level is essential** for two reasons:

1. **Correctness gate** — catch IR inconsistencies (bad types, out-of-scope variables, arity mismatches) before they produce broken C/C++/bytecode.
2. **Pass debugging** — every transformation pass (normalize, transform, codegen) can call the verifier before and after to verify that no invariant was broken (following the LLVM/MLIR model of per-pass verification).

The goal of this plan is to implement a **`FirModuleChecker`** that verifies a complete `Module` FIR node — including struct fields, global variables, and the list of functions — in a single multi-pass traversal.

---

## 2. Analysis of the C++ Checkers

### 2.1 `FIRTypeChecker` — What it does

```
FIRTypeChecker : DispatchVisitor
```

Visits the AST and checks three things:

| Instruction | Check | Response |
|---|---|---|
| `BinopInst` | Both operands have the same type (int/bool mixing tolerated) | `WARNING` + optional assert |
| `Select2Inst` | Condition is int or bool| `WARNING` + optional assert |
| `CastInst` | Source and target types are different (warns if cast to same type) | `WARNING` + optional assert |

**Limitations:**
- Uses `TypingVisitor::getType()` which requires `gGlobal` (eliminated in Rust)
- Does not check `Select2` branch type compatibility
- Does not check binop result type
- Does not check function call argument types
- Does not know the module context (struct, globals)

### 2.2 `FIRCodeChecker` — What it does

```
FIRCodeChecker : DispatchVisitor
VarScope = map<string, pair<AccessType, bool>>  // bool = initialized
```

Maintains a **stack of scopes** (`fStackVarsTable` + `fCurVarScope`) and checks:

| Check | Details |
|---|---|
| `LoadVar` — variable defined | Error if name not found in any scope |
| `LoadVar` — variable initialized | Error if `bool` flag is false (except `kFunArgs`) |
| `LoadVar` — access coherency | Error if `AccessType` of Load ≠ AccessType of declaration |
| `StoreVar` — variable defined | Error if name not found in any scope |
| `StoreVar` — access coherency | Error if `AccessType` of Store ≠ AccessType of declaration |
| `FunCall` — function declared | Error if name not in `fFunctionTable` |
| `FunCall` — arity | Error if arg count ≠ declared param count |
| `ForLoopInst` | Creates a new scope for init/end/body |
| `BlockInst` | Creates a new scope for all statements |

**Limitations:**
- No argument type checking for function calls (only arity)
- No return type checking
- No struct field cross-validation
- No `kStruct` variable vs struct declaration consistency
- No module-level completeness (required functions)
- Does not check `kLoop` variable isolation
- `DeclareFunInst` injects args in `fCurVarScope` but never pops them correctly (leaks function args into sibling function context)
- Counts errors but does not accumulate structured diagnostics

### 2.3 `FIRVarChecker` — What it does

```
FIRVarChecker : DispatchVisitor
fStackVariable : stack<map<string, AccessType>>
fStructVariable : map<string, AccessType>
```

Separates struct variable declarations from stack variables and checks scope coherence for named address accesses. More precise than `FIRCodeChecker` for struct variables but still operates on a single block, not a full module.

### 2.4 Summary of Gaps to Fill

| Gap | Priority |
|---|---|
| No module-level structural validation | High |
| No cross-function struct field consistency | High |
| No function argument type checking | High |
| No return type checking | High |
| `Select2` branch type compatibility | Medium |
| Binop result type validation | Medium |
| Loop variable type checks | Medium |
| Array/table index type checks | Medium |
| Duplicate symbol detection (vars, functions) | High |
| Required DSP API functions presence | Medium |
| No-op cast detection | Low (warning) |

---

## 3. Semantic Verification of an IR — Theory

Semantic verification of an intermediate representation covers everything that cannot be expressed by the syntactic grammar alone. It operates at several levels:

### 3.1 Levels of Verification (following LLVM and MLIR)

**Module level**
- The module is structurally complete (required sections present)
- No duplicate top-level symbol names

**Type system level (type checking)**
- Every expression has a deterministic type
- Operand types are compatible with the operation
- Assignment types match declared types

**Scope and dominance level**
- Every use of a variable is dominated by its declaration
- Every use of a variable is dominated by at least one initialization (definite assignment)
- Variables do not escape their lexical scope
- Access class of a use matches the access class of the declaration (`kStack` variable not loaded as `kStruct`)

**Control flow level**
- All code paths in a non-void function return a value
- Loop variables are of integer type
- Switch case labels are unique integers
- No unreachable code after `Return` (warning)

**Arity and signature level**
- Function call arguments match the declared parameter count
- Function call argument types match declared parameter types (with implicit promotion rules)
- Return type matches function signature

### 3.2 Rust IR verifiers and references

| System | Verifier | Notable checks |
|---|---|---|
| LLVM IR | `lib/IR/Verifier.cpp` | Both operands of binop have same type; value definitions dominate all uses; binary operator parameters are first-class types |
| MLIR | `mlir/IR/Verifier.cpp` | Per-operation type constraints expressed as traits/interfaces; run before and after every pass |
| GraalVM IR | Formal operational semantics (Bhat et al., 2024) | SSA dominance + type invariants proved in Lean |

**FIR is not in SSA form** (variables are mutable and can be stored/loaded multiple times), so dominance analysis is replaced by a simpler **scope + initialized-flag** analysis, as already done in `FIRCodeChecker`.

### 3.3 Key invariants specific to Faust FIR

1. **kStruct variables** are declared in the `dsp_struct` and accessed across all methods — they are the DSP state.
2. **kStack variables** are local to a function body and must not outlive their block.
3. **kLoop variables** are local to a loop and must not be accessed outside it.
4. **kFunArgs variables** are the formal parameters of a `DeclareFun` and are valid only within that function's body.
5. **kStatic / kGlobal variables** are declared in the `globals` block and accessed from any function.
6. The `compute` function has a canonical signature: `(int count, FAUSTFLOAT** inputs, FAUSTFLOAT** outputs) -> void`.
7. The DSP struct must contain at least the fields referenced by `kStruct` accesses inside function bodies.

---

## 4. FIR Module Structure Reminder

A `FirMatch::Module` node contains:

```
Module {
    name: String,           // DSP class name e.g. "mydsp"
    dsp_struct: FirId,      // → Block [ DeclareVar(kStruct), ... ]
    globals: FirId,         // → Block [ DeclareVar/DeclareTable(kStatic|kGlobal|kStaticStruct), DeclareFun(extern), ... ]
    declarations: FirId,    // → Block [ DeclareFun(...), DeclareFun(...), ... ]
}
```

The **struct fields** are the `kStruct`-access `DeclareVar` nodes inside `dsp_struct`.

The **globals** are `DeclareVar` / `DeclareTable` nodes (with `kStatic`, `kStaticStruct`, or `kGlobal` access) and may also contain prototype-only `DeclareFun` nodes (extern declarations used by calls in function bodies, e.g. math intrinsics).

The **declarations block** contains `DeclareFun` nodes. Each function has:
- A name
- A `FirType::Fun { args, ret }` signature
- An optional body (`Block` of statements)

Expected DSP API functions (may vary by backend):

| Function | Signature |
|---|---|
| `classInit` | `(int sample_rate) -> void` |
| `instanceConstants` | `(dsp*, int sample_rate) -> void` |
| `instanceResetUserInterface` | `(dsp*) -> void` |
| `instanceClear` | `(dsp*) -> void` |
| `instanceInit` | `(dsp*, int sample_rate) -> void` |
| `init` | `(dsp*, int sample_rate) -> void` |
| `buildUserInterface` | `(dsp*, UI*) -> void` |
| `getSampleRate` | `(dsp*) -> int` |
| `compute` | `(dsp*, int, FAUSTFLOAT**, FAUSTFLOAT**) -> void` |
| `metadata` | `(Meta*) -> void` |

---

## 5. Complete Inventory of Checks

Checks are classified by **severity** and **category**:
- `[E]` = Error (blocks code generation)
- `[W]` = Warning (suspicious but may be valid for some backends)

### 5.1 Module Structure

| # | Severity | Check |
|---|---|---|
| M01 | E | The root node is `FirMatch::Module` |
| M02 | E | `dsp_struct` decodes as `Block`|
| M03 | E | `globals` decodes as `Block` |
| M04 | E | `declarations` decodes as `Block` |
| M05 | E | All nodes in `declarations` are `DeclareFun` |
| M06 | W | No duplicate function names in `declarations` |
| M07 | W | Expected DSP API functions are present (`compute`, `buildUserInterface`, `metadata`, etc.) |

### 5.2 Struct Field Declarations

| # | Severity | Check |
|---|---|---|
| S01 | E | Each field is a `DeclareVar` with `kStruct` access |
| S02 | E | No duplicate field names |
| S03 | E | Field types are not `Void` or unknown |
| S04 | W | Array fields have size > 0 |

### 5.3 Global Variable Declarations

| # | Severity | Check |
|---|---|---|
| G01 | E | Each globals-block node is `DeclareVar`, `DeclareTable`, or prototype `DeclareFun` |
| G02 | E | Access type is `kStatic`, `kStaticStruct`, or `kGlobal` |
| G03 | E | No duplicate global names |
| G04 | W | Global initializer type matches declared type (if initializer present) |

### 5.4 Function Declarations

| # | Severity | Check |
|---|---|---|
| F01 | E | Function type is `FirType::Fun` |
| F02 | E | Return type is a valid `FirType` (not `Unknown`) |
| F03 | E | All parameter types are valid (not `Unknown`) |
| F04 | E | No duplicate parameter names within a function |
| F05 | W | Functions without a body that are not extern declarations |

### 5.5 Variable Scope Analysis (per function body)

Scope context during traversal:
- **struct scope** = fields registered in phase 1 (accessible anywhere as `kStruct`)
- **global scope** = globals registered in phase 1 (accessible anywhere as `kStatic`/`kGlobal`)
- **function args** = parameters of the current function (accessible as `kFunArgs`)
- **lexical scope stack** = stack of `HashMap<name, (AccessType, FirType, InitStatus)>`

`InitStatus` = `Uninitialized | Initialized | MaybeInitialized`

| # | Severity | Check |
|---|---|---|
| SC01 | E | Every `LoadVar` name is found in the active scope (struct, global, args, or lexical) |
| SC02 | E | Every `LoadVar` `AccessType` matches the scope in which the name was declared |
| SC03 | W | Every `LoadVar` with `kStack` access is `Initialized` or `MaybeInitialized` |
| SC04 | E | Every `StoreVar` name is found in the active scope |
| SC05 | E | Every `StoreVar` `AccessType` matches the declared scope |
| SC06 | E | `kLoop` variables are only accessible within their own `ForLoop` or `SimpleForLoop` scope |
| SC07 | E | `kFunArgs` variables are not declared inside the function body (only as function parameters) |
| SC08 | E | `kStack` variables declared in an inner block are not accessed in outer blocks |
| SC09 | W | `kStruct` variable names are declared in the DSP struct |

### 5.6 Binary Operations

| # | Severity | Check |
|---|---|---|
| B01 | E | Both operands have the same `FirType` (excluding int/bool mixing which is allowed) |
| B02 | E | Operand types are numeric (`Int32`, `Int64`, `Float32`, `Float64`, `Bool`) |
| B03 | W | Declared result `typ` field is consistent with operand types (follows C++ promotion rules) |
| B04 | W | Division (`Div`) with a right operand that is a constant `0` |

### 5.7 Unary Operations

| # | Severity | Check |
|---|---|---|
| U01 | E | `Neg`: operand type is numeric |
| U02 | W | `Cast`: source and target types differ (`typ == operand_type` is a no-op) |
| U03 | E | `Cast`: both source and target are numeric types (no pointer casts) |
| U04 | W | `Bitcast`: source and target have the same bit-width |

### 5.8 Conditional and Selection

| # | Severity | Check |
|---|---|---|
| C01 | E | `Select2` condition is `Int32`, `Int64`, or `Bool` |
| C02 | W | `Select2` then-branch and else-branch have the same `FirType` |
| C03 | W | `Select2` declared result `typ` matches branch types |
| C04 | E | `If` condition is `Int32`, `Int64`, or `Bool` |

### 5.9 Function Calls

| # | Severity | Check |
|---|---|---|
| FC01 | E | Called function name is in the function table (declared in module or extern) |
| FC02 | E | Argument count matches declared parameter count |
| FC03 | W | Argument types are compatible with declared parameter types (with implicit numeric promotion) |
| FC04 | W | Return type is used correctly (discarded or assigned to compatible type) |

### 5.10 Loops

| # | Severity | Check |
|---|---|---|
| L01 | E | `ForLoop` loop variable (`var`) has `kLoop` access |
| L02 | E | `ForLoop` loop variable type is `Int32` or `Int64` |
| L03 | E | `WhileLoop` condition type is `Int32`, `Int64`, or `Bool` |
| L04 | W | `ForLoop` body is non-empty |

### 5.11 Switch

| # | Severity | Check |
|---|---|---|
| SW01 | E | `Switch` condition type is `Int32` or `Int64` |
| SW02 | E | No duplicate case values |
| SW03 | W | At least one case is present |

### 5.12 Return

| # | Severity | Check |
|---|---|---|
| R01 | E | `Return(Some(val))` — inferred type of `val` matches current function's return type |
| R02 | W | `Return(None)` used in a non-void function |
| R03 | W | Statements after a `Return` in a block (dead code) |

### 5.13 Table and Array Access

| # | Severity | Check |
|---|---|---|
| T01 | E | `LoadTable` / `StoreTable` index type is `Int32` or `Int64` |
| T02 | E | `StoreTable` value type matches the table element type |
| T03 | W | `LoadTable` / `StoreTable` references a declared table (not an arbitrary variable) |

### 5.14 Math Calls

Math functions exposed via `FirMathOp` have well-known signatures. They have to be defined as pure function prototype in the module global section.
 These checks apply when a `FunCall` name matches a known math symbol (via `FirMathOp::from_symbol`):

| # | Severity | Check |
|---|---|---|
| MA01 | W | Unary math ops (`sin`, `cos`, `sqrt`, etc.) called with exactly 1 argument |
| MA02 | W | Binary math ops (`pow`, `fmod`, `atan2`, `min`, `max`) called with exactly 2 arguments |
| MA03 | W | Floating-point math ops called with float/double arguments (not int) |
| MA04 | W | `abs` / `fabs` distinction respected (int vs float) |

Each math function call should be checked by verifying that the corresponding prototype is correctly defined. 

---

## 6. Rust Architecture

### 6.1 Module layout

```
crates/fir/src/
├── lib.rs            (existing — re-exports checker module)
├── checker.rs        (NEW — FirModuleChecker and all verification logic)
```

Or, if the checker grows large enough to justify separation:
```
crates/fir-check/src/
├── lib.rs
├── context.rs        (VerifyContext, SymbolTable, ScopeStack)
├── type_infer.rs     (FirTypeInferrer)
├── module.rs         (module-level checks)
├── scope.rs          (scope analysis)
├── types.rs          (type consistency checks)
├── control.rs        (loops, switch, return checks)
└── diagnostics.rs    (FirDiagnostic types)
```

Initial target: a single `checker.rs` module inside `crates/fir`, promoted to a separate crate later if needed.

### 6.2 Public API

```rust
/// Entry point: verify a complete FIR module.
pub fn verify_fir_module(
    store: &FirStore,
    module_id: FirId,
) -> FirVerifyReport

/// Verify a single function body (useful for per-pass lightweight checks).
pub fn verify_fir_function(
    store: &FirStore,
    fun_id: FirId,
    ctx: &ModuleSymbols,
) -> FirVerifyReport
```

### 6.3 Key data structures

```rust
/// Result of a full verification run.
pub struct FirVerifyReport {
    pub diagnostics: Vec<FirDiagnostic>,
}

impl FirVerifyReport {
    pub fn has_errors(&self) -> bool { ... }
    pub fn errors(&self) -> impl Iterator<Item = &FirDiagnostic> { ... }
    pub fn warnings(&self) -> impl Iterator<Item = &FirDiagnostic> { ... }
    /// Panic with a formatted report if any errors are present (for debug builds).
    pub fn assert_ok(&self) { ... }
}

/// A single diagnostic item.
pub struct FirDiagnostic {
    pub severity: Severity,
    pub code: &'static str,        // e.g. "FIR-SC01", "FIR-B01"
    pub message: String,
    pub node: FirId,               // FirId of the offending node (for context)
    pub context: DiagContext,      // enclosing function name, variable name, etc.
}

pub enum Severity { Error, Warning }

#[derive(Default)]
pub struct DiagContext {
    pub function_name: Option<String>,
    pub variable_name: Option<String>,
}
```

### 6.4 Internal context — `VerifyCtx`

```rust
/// Mutable context threaded through the whole verification traversal.
struct VerifyCtx<'s> {
    store: &'s FirStore,
    diags: Vec<FirDiagnostic>,

    // Phase 1 results — module-level symbol tables
    symbols: ModuleSymbols,

    // Phase 3 — per-function state
    current_function: Option<String>,
    current_return_type: Option<FirType>,
    scope_stack: ScopeStack,
}

/// Symbol tables populated during phase 1 (module-level pass).
struct ModuleSymbols {
    /// DSP struct fields: name → FirType
    struct_fields: HashMap<String, FirType>,

    /// Global/static vars: name → (AccessType, FirType)
    globals: HashMap<String, (AccessType, FirType)>,

    /// Declared functions: name → FunctionSig
    functions: HashMap<String, FunctionSig>,
}

struct FunctionSig {
    params: Vec<(String, FirType)>,
    return_type: FirType,
    is_extern: bool,
}
```

### 6.5 Scope stack — `ScopeStack`

```rust
struct ScopeStack {
    frames: Vec<ScopeFrame>,
}

struct ScopeFrame {
    kind: FrameKind,
    vars: HashMap<String, VarEntry>,
}

enum FrameKind {
    Block,
    Loop { var_name: String },
    Function,
}

struct VarEntry {
    access: AccessType,
    typ: FirType,
    init: InitStatus,
}

#[derive(Clone, Copy, PartialEq)]
enum InitStatus {
    No,
    Yes,
    Maybe,   // after conditional branches
}
```

**Scope resolution order** (for a `LoadVar` with name `N` and access `A`):
1. If `A == kStruct` → look in `symbols.struct_fields`
2. If `A == kStatic || A == kGlobal || A == kStaticStruct` → look in `symbols.globals`
3. If `A == kFunArgs` → look in function params of `current_function`
4. If `A == kStack || A == kLoop` → walk `scope_stack` from top
5. Not found → error SC01

### 6.6 Type inference — `infer_type`

```rust
fn infer_type(store: &FirStore, id: FirId, ctx: &VerifyCtx) -> Option<FirType>
```

Follows the C++ `TypingVisitor` rules (from `typing_instructions.hh`):

| Node | Inferred type |
|---|---|
| `Int32 { .. }` | `FirType::Int32` |
| `Int64 { .. }` | `FirType::Int64` |
| `Float32 { .. }` | `FirType::Float32` |
| `Float64 { .. }` | `FirType::Float64` |
| `Bool { .. }` | `FirType::Bool` |
| `BinOp { op, lhs, rhs, typ }` | Use `typ` field (declared type) and cross-check with operand types |
| `Cast { typ, .. }` | `typ` |
| `Select2 { then_value, typ, .. }` | `typ` (or infer from then branch) |
| `FunCall { name, typ, .. }` | `typ` (or look up function return type in symbol table) |
| `LoadVar { name, access, typ }` | Look up in scopes; cross-check with `typ` |
| `Neg { typ, .. }` | `typ` |
| `NullValue { typ }` | `typ` |

**Important:** All FIR value nodes carry an explicit `typ` field. `infer_type` trusts this field for efficiency but cross-checks it against operand types to detect inconsistencies (the source of most type errors).

---

## 7. Implementation Phases

### Phase 1 — Module structure and symbol collection *(~2 days)*

**Goal:** Validate the module skeleton and populate `ModuleSymbols`.

Steps:
1. Decode root node → check M01–M04
2. Walk `dsp_struct` → register struct fields, check S01–S04
3. Walk `globals` block → register globals, check G01–G04
4. Walk `declarations` block → register function signatures, check F01–F07, M06–M07

No type inference required. No scope tracking required. Output: `ModuleSymbols` + first batch of diagnostics.

### Phase 2 — Per-function scope analysis *(~3 days)*

**Goal:** Implement `ScopeStack` and verify variable declarations, accesses, and control flow.

Steps:
1. For each `DeclareFun` in declarations:
   - Set `current_function` and `current_return_type`
   - Push function frame with `kFunArgs` vars
   - Traverse body with scope-aware visitor
2. On `DeclareVar`:
   - Insert into current frame with `InitStatus::No` (or `Yes` if initializer present)
   - Check access type is `kStack` or `kLoop`
3. On `StoreVar`:
   - Look up via scope resolution → check SC04, SC05
   - Mark as `InitStatus::Yes`
4. On `LoadVar`:
   - Look up via scope resolution → check SC01, SC02, SC03
5. On `Block`:
   - Push `FrameKind::Block`, traverse children, pop frame
6. On `ForLoop` / `SimpleForLoop`:
   - Push `FrameKind::Loop { var_name }`, traverse, pop frame
   - Check L01–L04
7. On `WhileLoop`:
   - Check L03
8. On `Return`:
   - Check R01–R03 using `current_return_type`
9. On `Switch`:
   - Check SW01–SW03
10. On `IfInst`:
    - Traverse then/else branches each with a new block frame
    - Merge `InitStatus` (use `Maybe` for vars initialized in only one branch)

### Phase 3 — Type checking *(~3 days)*

**Goal:** Verify type consistency of all expressions.

Steps:
1. Implement `infer_type` for all value nodes
2. On `BinOp`:
   - Infer lhs and rhs types
   - Check B01–B04
3. On `Neg`, `Cast`, `Bitcast`:
   - Check U01–U04
4. On `Select2`:
   - Check C01–C03
5. On `FunCall`:
   - Check FC01–FC04
6. On `LoadVar` / `StoreVar`:
   - Cross-check declared type with resolved scope type
7. On `LoadTable` / `StoreTable`:
   - Check T01–T03
8. On math function calls:
   - Detect via `FirMathOp::from_symbol`
   - Check MA01–MA04

### Phase 4 — Integration and polish *(~1 day)*

1. Export `verify_fir_module` from `crates/fir/src/lib.rs`
2. Wire into `compiler` crate: call verifier after FIR generation, before codegen
3. Emit `FirVerifyReport` entries and let `compiler` map them to shared diagnostics
4. Add `--verify-fir` CLI flag to enable/disable (enabled by default in debug builds)
5. Write integration tests (see Section 9)

---

## 8. Integration in the Pipeline

```
Signals
  → transform::signal_to_fir  →  FIR Module
                                     │
                                     ▼
                              FirModuleChecker::verify()   ← NEW (Phase 4 integration)
                                     │
                                  if ok ──→ codegen (C, C++, Interp, Rust...)
                                     │
                                  if errors → emit diagnostics, abort codegen
```

Additionally, individual transformation passes that modify FIR can call `verify_fir_function` as a lightweight post-pass sanity check (similar to MLIR's per-operation verifier pattern).

### CLI integration

```bash
# Default: verifier enabled (errors abort, warnings printed)
faust-rs -lang cpp foo.dsp

# Strict mode: warnings also abort
faust-rs -lang cpp --fir-verify-strict foo.dsp

# Disable verifier (for benchmarking or trusted IR)
faust-rs -lang cpp --no-fir-verify foo.dsp

# Dump verification report without codegen
faust-rs --dump-fir-verify foo.dsp
```

---

## 9. Test Strategy

### 9.1 Unit tests (in `crates/fir/src/checker.rs`)

For each check group, a test that:
1. Builds a minimal valid FIR module using `FirBuilder` → asserts `report.has_errors() == false`
2. Introduces the specific violation → asserts `report.has_errors() == true` and the correct diagnostic code is present

**Examples:**

```rust
#[test]
fn test_b01_binop_type_mismatch() {
    let mut store = FirStore::new();
    let mut b = FirBuilder::new(&mut store);
    let i = b.int32(42);
    let f = b.float32(1.0);
    // Int32 + Float32 → type mismatch
    let bad_binop = b.binop(FirBinOp::Add, i, f, FirType::Int32);
    let fun = b.declare_fun("test", FirType::Fun { args: vec![], ret: Box::new(FirType::Void) },
                            &[], b.block(&[b.drop_(bad_binop)]), false);
    let module = build_minimal_module(&mut store, &mut b, vec![fun]);
    let report = verify_fir_module(&store, module);
    assert!(report.has_errors());
    assert!(report.diagnostics.iter().any(|d| d.code == "FIR-B01"));
}
```

### 9.2 Integration tests

In `tests/`, create a corpus of `.dsp` files compiled to FIR that should pass verification:

```bash
cargo run -p xtask -- verify-fir-corpus
```

This ensures that all real DSP programs generate well-formed FIR.

### 9.3 Negative test corpus

A set of hand-crafted invalid FIR modules (one per check) stored as Rust constants, each expected to trigger a specific diagnostic code. Run via:

```bash
cargo test -p fir -- checker
```

### 9.4 Golden test extension

The existing `xtask golden-check` infrastructure can be extended to also record the `FirVerifyReport` (error count = 0) as part of the golden metadata.

---

## 10. Relationship with `diagnostics` Crate

The implemented boundary keeps `FirVerifyReport` and `FirDiagnostic` in
`crates/fir`. The `compiler` facade converts the report into the shared
`DiagnosticBundle` with stable `FRS-FIR-*` codes. This preserves FIR checker
independence while keeping one public rendering and tooling schema; `fir` does
not depend directly on `diagnostics`.

---

## Appendix A — Diagnostic Code Registry

| Code | Severity | Description |
|---|---|---|
| FIR-M01 | E | Root node is not a Module |
| FIR-M02 | E | dsp_struct is not a struct type declaration |
| FIR-M03 | E | globals is not a Block |
| FIR-M04 | E | declarations is not a Block |
| FIR-M05 | E | Non-DeclareFun node in declarations block |
| FIR-M06 | W | Duplicate function name |
| FIR-M07 | W | Missing expected DSP API function |
| FIR-S01 | E | Struct field is not DeclareVar(kStruct) |
| FIR-S02 | E | Duplicate struct field name |
| FIR-S03 | E | Struct field has void type |
| FIR-S04 | W | Struct array field has size 0 |
| FIR-G01 | E | Globals-block node is not DeclareVar / DeclareTable / DeclareFun |
| FIR-G02 | E | Global declaration has wrong access type |
| FIR-G03 | E | Duplicate global variable name |
| FIR-G04 | W | Global initializer type mismatch |
| FIR-F01 | E | Function type is not FirType::Fun |
| FIR-F02 | E | Function return type is unknown |
| FIR-F03 | E | Function parameter type is unknown |
| FIR-F04 | E | Duplicate parameter name in function |
| FIR-F05 | W | compute return type is not Void |
| FIR-F06 | W | compute parameter count is not 4 |
| FIR-F07 | W | Non-extern function has no body |
| FIR-SC01 | E | LoadVar of undeclared variable |
| FIR-SC02 | E | LoadVar access type inconsistency |
| FIR-SC03 | W | LoadVar of uninitialized stack variable |
| FIR-SC04 | E | StoreVar to undeclared variable |
| FIR-SC05 | E | StoreVar access type inconsistency |
| FIR-SC06 | E | kLoop variable used outside its loop |
| FIR-SC07 | E | kFunArgs variable redeclared in body |
| FIR-SC08 | E | kStack variable used outside its block |
| FIR-SC09 | W | kStruct variable not in struct declaration |
| FIR-B01 | E | BinOp operand type mismatch |
| FIR-B02 | E | BinOp operand is not numeric |
| FIR-B03 | W | BinOp result type inconsistent with operands |
| FIR-B04 | W | BinOp division by constant zero |
| FIR-U01 | E | Neg of non-numeric operand |
| FIR-U02 | W | Cast is a no-op (source == target type) |
| FIR-U03 | E | Cast between non-numeric types |
| FIR-U04 | W | Bitcast between types of different width |
| FIR-C01 | E | Select2 condition is not int or bool |
| FIR-C02 | W | Select2 branch type mismatch |
| FIR-C03 | W | Select2 result type inconsistent with branches |
| FIR-C04 | E | If condition is not int or bool |
| FIR-FC01 | E | Call to undeclared function |
| FIR-FC02 | E | Function call arity mismatch |
| FIR-FC03 | W | Function call argument type mismatch |
| FIR-FC04 | W | Function return value type mismatch at use site |
| FIR-L01 | E | ForLoop variable has wrong access type |
| FIR-L02 | E | ForLoop variable is not integer type |
| FIR-L03 | E | WhileLoop condition is not int or bool |
| FIR-L04 | W | ForLoop body is empty |
| FIR-SW01 | E | Switch condition is not integer |
| FIR-SW02 | E | Duplicate switch case value |
| FIR-SW03 | W | Switch has no cases |
| FIR-R01 | E | Return value type does not match function return type |
| FIR-R02 | W | Return without value in non-void function |
| FIR-R03 | W | Dead code after Return statement |
| FIR-T01 | E | Table index is not integer |
| FIR-T02 | E | StoreTable value type does not match element type |
| FIR-T03 | W | LoadTable/StoreTable on non-table variable |
| FIR-MA01 | W | Unary math op called with wrong arity |
| FIR-MA02 | W | Binary math op called with wrong arity |
| FIR-MA03 | W | Float math op called with integer argument |
| FIR-MA04 | W | abs/fabs int/float distinction violated |

---

## Appendix B — Mapping to C++ Checkers

| C++ check | C++ class | Rust equivalent |
|---|---|---|
| BinopInst type mismatch | `FIRTypeChecker::visit(BinopInst*)` | FIR-B01 |
| Select2Inst condition type | `FIRTypeChecker::visit(Select2Inst*)` | FIR-C01 |
| CastInst no-op | `FIRTypeChecker::visit(CastInst*)` | FIR-U02 |
| LoadVar — undefined | `FIRCodeChecker::visit(LoadVarInst*)` | FIR-SC01 |
| LoadVar — uninitialized | `FIRCodeChecker::visit(LoadVarInst*)` | FIR-SC03 |
| LoadVar — access incoherency | `FIRCodeChecker::visit(LoadVarInst*)` | FIR-SC02 |
| StoreVar — undefined | `FIRCodeChecker::visit(StoreVarInst*)` | FIR-SC04 |
| StoreVar — access incoherency | `FIRCodeChecker::visit(StoreVarInst*)` | FIR-SC05 |
| FunCall — undeclared | `FIRCodeChecker::visit(FunCallInst*)` | FIR-FC01 |
| FunCall — arity | `FIRCodeChecker::visit(FunCallInst*)` | FIR-FC02 |

All other checks in this document are **new** and have no C++ equivalent.

---

*Sources:*
- [MLIR Verifiers — Jeremy Kun](https://www.jeremykun.com/2023/09/13/mlir-verifiers/)
- [LLVM IR Verifier source](https://github.com/llvm/llvm-project/blob/main/llvm/lib/IR/Verifier.cpp)
- [Semantic Analysis in Compiler Design — GeeksforGeeks](https://www.geeksforgeeks.org/compiler-design/semantic-analysis-in-compiler-design/)
- [Intermediate Representation — ACM Queue](https://queue.acm.org/detail.cfm?id=2544374)
- C++ source: `compiler/generator/fir_to_fir.hh`, `compiler/generator/fir/fir_code_checker.hh`, `compiler/generator/typing_instructions.hh`
