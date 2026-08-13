# Cranelift Backend Plan (Faust Rust Port)

**Date:** 2026-02-27  
**Status:** Active implementation; scalar JIT/runtime and `mem0` complete for the supported subset

**Target crates:** `codegen`, `compiler`, `xtask`, `cranelift-ffi`  
**Primary backend module (planned):** `codegen::backends::cranelift`

---

## 1. Purpose and Positioning

This document defines a detailed implementation plan for a new **Cranelift-based
backend** in `faust-rs`.

This backend is **not a direct C++ parity backend** (unlike C/C++/Interp/Wasm/LLVM
paths that map to existing Faust backends). It is a **Rust-native extension**
that compiles Faust FIR to native machine code via the Cranelift code generator.

### Current implementation snapshot (2026-08-13)

What is already in place:

- `codegen::backends::cranelift` has a real Cranelift JIT integration.
- FIR lowering covers a non-trivial executable subset (`Block`, `DeclareVar`,
  `If`, `Switch`, `SimpleForLoop`, `ForLoop`, `WhileLoop`, `StoreVar`,
  `StoreTable`, `LoadVar`, `LoadTable`, several math calls, etc.).
- `compiler` CLI has experimental integration (`--dump-cranelift`,
  `-lang cranelift`).
- `cranelift-ffi` can compile through the compiler facade and stores a compiled
  `JitDspModule` in factory objects.
- The instance runtime executes finalized JIT lifecycle/compute functions and
  dispatches UI/metadata through the retained sidecar.
- File, string, signal, and box factory paths plus source-backed persistence
  are implemented.
- The scalar mode-zero memory-manager path is complete as detailed below.

What is still blocking v1 completion:

- Subset fallback policy (`compute` stub on unsupported FIR) is still used; v1
  completion requires explicit corpus-level coverage and strict diagnostics
  policy.
- LLVM-specific target, IR, machine, and object APIs remain deliberately
  deferred, and the source-backed payload is not a native-code snapshot.

### `mem0` implementation update (2026-08-13)

The scalar Cranelift path now implements the mode-zero custom memory-manager
family described by
`custom-memory-manager-mem0-analysis-and-porting-plan-2026-08-13-en.md`:

- `CraneliftOptions::memory_manager_mode` retains the typed mode in JIT output,
  cache identity, compile options, strict JSON, and source-backed persistence;
- eligible instance arrays use pointer slots in `StructLayoutPlan`, while
  writable generated tables use factory-owned class pointer slots;
- `cranelift-ffi` binds the shared aligned `faust_memory_manager` ABI, performs
  transactional allocations, deep clone, and reverse instance/class release;
- C and C++ factory set/get surfaces are functional; restored factories are
  deliberately unbound because callbacks and addresses are never serialized;
- version-2 JSON exposes the effective target layout and backend-neutral
  `compute_cost` before allocation;
- `tests/impulse-tests/Make.mem0` runs strict lowering, ownership audits,
  ordinary/mem0 numeric parity, cross-backend JSON parity, and optimization
  levels 0/3.

This is an `adapted` Rust extension: upstream Faust has no Cranelift backend.
Only scalar `mem0` is in scope; `mem1`–`mem3` and vector custom-memory lowering
remain unsupported.

### Why add a Cranelift backend?

- Fast native code generation without LLVM toolchain complexity.
- Good fit for JIT and AOT object emission from Rust.
- Useful for runtime validation and future low-latency embedding scenarios.
- Reduces dependency/packaging friction compared to LLVM in many environments.

### Mapping status (public API policy)

- **C++ source symbol mapping**: `deferred` (no direct C++ Cranelift backend exists).
- **Rust backend API mapping**: `adapted` (new backend-specific APIs and options).
- **External C/C++ runtime API mapping (`cranelift_dsp`)**: `1:1` target for exported function set and cache/factory strategy (modeled on `llvm_dsp` / `interpreter_dsp` APIs).
- **Compatibility impact**: additive only; existing backends and CLI behavior remain unchanged by default.

---

## 2. Scope, Goals, and Non-Goals

## 2.1 Primary Goals (v1)

1. Compile a validated FIR `Module` into **native code** using Cranelift.
2. Support the canonical Faust DSP lifecycle entry points needed for execution:
   - `compute`
   - `instanceConstants`, `instanceResetUserInterface`, `instanceClear`
   - `buildUserInterface`
   - `metadata`
3. Provide one execution mode for validation:
   - **JIT mode** (in-process execution)
4. Integrate into Rust validation workflows for differential checking against `interp`.
5. Expose a **C and C++ `cranelift_dsp` runtime/factory API** in V1 (similar in role to
   `llvm_dsp` and `interpreter_dsp` exports).
6. Keep the implementation isolated behind a feature flag and explicit API.

## 2.2 Secondary Goals (v2+)

1. **AOT object emission** (`.o`) via `cranelift-object`.
2. Host-call support for a broader intrinsic set (math and utility externs).
3. Fast-lane differential runtime checks (`interp` vs `cranelift`) on selected corpus cases.
4. Optional backend CLI surface (`--dump-cranelift-ir` / `--run-cranelift` / object emit).
5. Additional FFI surfaces beyond the core `cranelift_dsp` runtime/factory contract.

## 2.3 Non-Goals (v1)

- Full replacement of the LLVM backend.
- Vectorized/SIMD FIR lowering parity on day 1.
- Cross-platform ABI-perfect shared library generation on day 1 (functional parity first).
- Real-time hard RT guarantees / lock-free runtime design.

---

## 3. Phase 0 Gate (Mandatory Before Deep Implementation)

This scope touches a critical backend/runtime path. Before implementation beyond
prototypes, confirm the Phase 0 items (`porting/phases/phase-0-validation-en.md`)
for the Cranelift backend specifically:

1. **Effective compile pipeline confirmation**
   - Use the current canonical Rust path:
   - `parse -> eval -> propagate -> normalize/type/interval -> transform -> fir -> backend`
   - In practice today: `parse -> eval -> propagate -> (transform fast-lane or legacy bridge) -> fir -> codegen`

2. **Differential baseline corpus and acceptance rules**
   - Define an initial runtime corpus subset (same shape as `interp` trace subset).
   - Define numeric tolerances and expected-skip policy.

3. **`gGlobal` decomposition plan relevance**
   - Backend must not reintroduce hidden global state.
   - Explicitly define per-compilation and per-JIT-instance ownership.

4. **TreeArena / FIR performance validation relevance**
   - No direct TreeArena redesign required, but verify compile overhead of FIR→Cranelift lowering on representative cases.

5. **API lifecycle and ownership model clarity**
   - Define ownership/lifetime of generated code, module, and callable entry points.
   - Define thread-safety guarantees (Send/Sync) for JIT handles and instances.

**Pass criterion (Phase 0 gate):**
- A short design note/checklist section in the implementation PR references this plan and confirms all five items.

---

## 4. Backend Architecture (Proposed)

## 4.1 Crate/module layout

Proposed layout in `codegen`:

```text
crates/codegen/src/backends/cranelift/
  mod.rs              # public backend entry points + options/errors
  lower.rs            # FIR -> Cranelift IR lowering
  types.rs            # FIR type -> Cranelift type/signature mapping
  abi.rs              # DSP ABI conventions / signature builders
  intrinsics.rs       # host-call declarations (math, min/max, etc.)
  jit.rs              # JIT orchestration (cranelift-jit)
  object.rs           # AOT object emission (v2, cranelift-object)
  value_state.rs      # variable slots / stack / address mapping
  tests.rs            # backend-focused unit tests
```

Proposed FFI crate (v1 requirement):

```text
crates/cranelift-ffi/
  src/lib.rs         # exported crate entry points (C ABI)
  src/factory.rs     # factory lifecycle, compile/create, cache
  src/instance.rs    # DSP instance lifecycle + compute
  src/types.rs       # opaque pointers, callback tables, alloc helpers
  src/cache.rs       # factory cache (SHA -> factory ptr)
  src/ui.rs          # UI/meta glue dispatch (if reused from interp pattern)
  include/
    cranelift-dsp-c.h  # C API
    cranelift-dsp.h    # C++ wrappers/classes (cranelift_dsp, factory)
```

Reference implementation style:
- `porting/faust-rust-ffi-interp-en.md`
- `crates/interp-ffi` (existing Rust FFI export patterns and cache/lifecycle model)

## 4.2 Feature flags

Proposed feature flags:

- `codegen/backend-cranelift` (new)
- Optional decomposition if needed later:
  - `codegen/backend-cranelift-jit`
  - `codegen/backend-cranelift-object`

Compiler re-export (later):

- `compiler` feature `backend-cranelift = ["codegen/backend-cranelift"]`

## 4.3 Public Rust API (v1 sketch, adapted)

```rust
pub struct CraneliftOptions {
    pub opt_level: CraneliftOptLevel,
    pub target_triple: Option<String>,
    pub enable_nan_canonicalization: bool,
    pub debug_ir_dump: bool,
}

pub enum CraneliftBackendErrorCode { /* stable codes */ }
pub struct CraneliftBackendError { /* code + message + context */ }

pub struct JitDspModule { /* compiled code + metadata + trampolines */ }
pub struct JitDspInstance { /* state buffer + function pointers */ }

pub fn compile_fir_to_cranelift_jit(
    store: &fir::FirStore,
    module: fir::FirId,
    options: &CraneliftOptions,
) -> Result<JitDspModule, CraneliftBackendError>;
```

This Rust API remains explicit and Rust-first, but **V1 also requires a C/C++
export surface** (`cranelift_dsp`) layered on top of it.

### 4.4 FFI/API surface requirement for V1 (`cranelift_dsp`)

V1 must expose a C/C++ API family with the **same exported function set and the
same factory/cache strategy** as the existing Faust runtime families:

- `llvm_dsp` / `llvm_dsp_factory`
- `interpreter_dsp` / `interpreter_dsp_factory`

V1 FFI deliverables (contract requirement):

- C API (`cranelift-dsp-c.h`) exposing the **same function families** as the
  reference runtime APIs (factory creation/read/write/cache/lifecycle/instance/UI/meta)
- C++ wrapper API (`cranelift-dsp.h`) exposing `cranelift_dsp` /
  `cranelift_dsp_factory` classes with the **same usage strategy** as
  `llvm_dsp` / `interpreter_dsp`
- factory creation from file/string (compile path)
- instance creation + full V1 lifecycle callable paths (`init`, `compute`,
  `buildUserInterface`, `metadata`, required reset/constants methods)
- factory deletion / instance deletion
- factory cache operations with the **same strategy and exported entry points**
  as the reference families (not optional)

API mapping status for this surface:
- **external compatibility surface**: `1:1 target` for exported function set,
  lifecycle strategy, and cache strategy (behavioral and API-shape parity target)

---

## 5. Runtime / ABI Contract (Critical Design Decision)

## 5.1 Recommended v1 strategy: internal backend ABI + external `cranelift_dsp` compatibility layer

For V1, the implementation may use an internal Rust-native executable ABI for
JIT/lowering, **but it must be wrapped by an exported C/C++ `cranelift_dsp`
surface** compatible in lifecycle expectations with `llvm_dsp` and
`interpreter_dsp`.

This yields two layers:

1. **internal backend ABI** (Rust/Cranelift lowering + JIT trampolines)
2. **external compatibility layer** (`cranelift-ffi` C API + C++ wrappers)

Recommended minimal callable target:

- `compute(dsp_ptr, count, inputs, outputs) -> void`

Where:
- `dsp_ptr` points to a Rust-owned state buffer representing `kStruct` fields.
- globals/static storage is owned by the compiled module or instance runtime.
- `inputs` / `outputs` follow the same shape as existing backends:
  `FAUSTFLOAT**`-style (`*const *const f32`, `*mut *mut f32`) for parity.

### External FFI contract (V1)

The exported `cranelift_dsp` layer must mirror the existing Faust runtime
families (`llvm_dsp`, `interpreter_dsp`) for:

- factory object that owns compiled module/JIT code
- DSP instance object with stable lifecycle methods
- one factory -> multiple instances
- explicit deletion functions (no hidden global ownership)
- factory cache keyed by source/options hash using the same strategy and
  exported cache-management function set

**Phase-0 requirement:** produce a function-by-function parity matrix
(`llvm/interpreter` exports -> `cranelift` exports) before implementation deepens.

**Reference priority for the parity matrix (locked):**

- **Primary reference:** `llvm_dsp` / `llvm_dsp_factory` (C and C++ exports)
- **Secondary cross-check:** `interpreter_dsp` / `interpreter_dsp_factory`

If `llvm_dsp` and `interpreter_dsp` differ, use `llvm_dsp` as the default
target and document the divergence explicitly in the parity matrix and
`JOURNAL.md` before implementation proceeds on the affected function family.

**C API naming convention (locked):**

- Use the interpreter-style backend-prefixed naming pattern for Cranelift C API
  functions (examples):
  - `createCCraneliftDSPFactoryFromFile`
  - `createCCraneliftDSPFactoryFromString`
  - `createCCraneliftDSPInstance`
- The exported function **set/strategy** remains a parity target against the
  `llvm_dsp` family; only the backend-specific naming prefix follows the
  `interpreter_dsp` style.
- Apply this interpreter-style backend prefixing rule consistently to Cranelift
  helper/query functions as well (for example LLVM-only generic C helpers),
  rather than reusing LLVM generic symbol names.

**C API source-creation signature policy (locked):**

- For `createCCraneliftDSPFactoryFromFile/String/Signals/Boxes`:
  - keep `opt_level` if Cranelift optimization levels are exposed,
  - do **not** carry the LLVM-specific `target` string parameter.
- This is an intentional ABI adaptation relative to `llvm_dsp` signatures while
  preserving function-family parity.

**V1 defer (locked):**

- Cranelift target getter/query functions corresponding to LLVM target-specific
  concepts (for example factory/machine target string getters) are deferred in
  V1 and tracked in the FFI parity matrix as `v1-deferred`.
- Memory-manager and foreign-function registration families (C and C++) are
  deferred in V1 and tracked in the FFI parity matrix as `v1-deferred`.
- LLVM-only IR/machine/object serialization families are deferred in V1
  **without exported symbols** (tracked in the FFI parity matrix as
  `v1-deferred`).

## 5.2 DSP state ownership (v1)

- `JitDspModule` owns compiled machine code and metadata (field layout, function registry).
- `JitDspInstance` owns:
  - state buffer (`kStruct` fields)
  - mutable globals if required by FIR lowering model
  - runtime scratch if needed

No hidden globals. Multiple instances from one module must be supported.

## 5.3 Function support levels

### v1 required

- `compute`
- `init` or equivalent exported initialization path sufficient for C/C++ wrappers
- exported function parity for the runtime/factory/cache C API family (same set
  as `llvm_dsp` / `interpreter_dsp`, even if some entries are temporarily
  implemented as typed "unsupported" during bring-up)
- enough lifecycle support for executable `cranelift_dsp` instances from C/C++
  (at least `instanceConstants`/`instanceResetUserInterface`/`instanceClear` if
  wrappers call them)
- `buildUserInterface`
- `metadata`
- enough FIR statement/value support to run selected runtime corpus cases

### v1.1 recommended

- `instanceInit` (if not already provided as part of the V1 wrapper contract)
- `getSampleRate`

### v2

- full lifecycle parity set (`init`, `instanceInit`, `buildUserInterface`, `metadata`, etc.) where applicable

---

## 6. FIR → Cranelift Lowering Scope

## 6.1 Lowering principle

Consume FIR exclusively through the canonical API:

- construction side: `FirBuilder`
- inspection side: `match_fir`

No backend-specific ad-hoc FIR decoding ladders.

## 6.2 Initial FIR subset (v1)

The backend should start with a **strictly enumerated executable subset**
covering the runtime corpus smoke cases.

### Values (v1 subset)

- constants: `Int32`, `Int64`, `Float32`, `Float64`, `Bool`
- `LoadVar`
- `LoadTable` (for inputs/outputs and selected arrays)
- `BinOp` (arithmetic/comparison subset needed by corpus)
- `Neg`
- `Cast` / `Bitcast` (only types validated by FIR checker and corpus needs)
- `Select2`
- `FunCall` (host intrinsic calls, curated allowlist)

### Statements (v1 subset)

- `DeclareVar`
- `StoreVar`
- `StoreTable`
- `Block`
- `If`
- `SimpleForLoop`
- `ForLoop`
- `Return`
- `Drop`
- `Label` (ignored/debug only)
- UI/meta statements used by `buildUserInterface` / `metadata` functions:
  - `OpenBox`
  - `CloseBox`
  - `AddButton`
  - `AddSlider`
  - `AddBargraph`
  - `AddSoundfile`
  - `AddMetaDeclare`

### Deferred in v1 unless required by chosen corpus

- `IteratorForLoop`
- `Switch`
- `WhileLoop`
- advanced soundfile runtime loading semantics and non-trivial table features
- exotic numeric types (`Quad`, `FixedPoint`)

## 6.3 Unsupported FIR policy

Unsupported FIR must return a typed backend error:

- stable error code family (e.g. `FRS-CGEN-CLIF-xxxx`)
- include FIR node kind and, when possible, function name
- no silent fallback to another backend inside `codegen`

---

## 7. Cranelift Integration Strategy

## 7.1 Recommended crates (v1)

- `cranelift-codegen`
- `cranelift-frontend`
- `cranelift-module`
- `cranelift-jit`
- `target-lexicon`

v2:
- `cranelift-object`

## 7.2 IR generation model

Use `FunctionBuilder` + explicit blocks:

- one Cranelift function per FIR `DeclareFun` with body
- maintain a backend lowering context per function:
  - variable map (`String` + `AccessType` -> Cranelift storage)
  - block stack
  - loop metadata
  - declared function signatures

## 7.3 Variable/storage mapping (v1)

Define explicit mapping rules:

- `kFunArgs` -> Cranelift function parameters
- `kLoop` -> Cranelift variables (frontend `Variable`)
- `kStack` -> Cranelift variables or stack slots (decision per type/mutability)
- `kStruct` -> loads/stores via `dsp_ptr + field_offset`
- `kStatic/kGlobal` -> backend-managed data slots (module/instance memory model)

Document the exact offset layout generation for `kStruct`.

## 7.4 Intrinsics/extern calls

Cranelift does not natively provide all math intrinsics used by Faust.

v1 strategy:
- define a curated host intrinsic table in Rust (`intrinsics.rs`)
- map FIR `FunCall` names to:
  - Cranelift libcall where available, or
  - imported host function symbol/trampoline

Pass criteria for this step:
- explicit allowlist + unsupported error path
- deterministic behavior on selected corpus subset

## 7.4.1 UI/meta callback host calls (V1 required)

`buildUserInterface` and `metadata` require host callback invocation through the
exported C API / C++ wrappers. V1 must define and implement callback trampolines for:

- UI glue callbacks (open/close boxes, sliders, buttons, bargraphs)
- UI soundfile callback (`add_soundfile`) dispatch path
- metadata declare callback

Recommended approach:

- follow the callback-table/opaque-pointer pattern already used in `interp-ffi`
- keep callback ABI handling in `cranelift-ffi` while Cranelift-generated code
  calls stable Rust trampolines

Pass criteria:

- `buildUserInterface` can drive a test `UIGlue` recorder and produce expected events
- `metadata` can drive a test `MetaGlue` recorder and produce expected key/value pairs

## 7.5 FFI export integration strategy (V1 requirement)

Recommended implementation layering:

- `codegen::backends::cranelift`
  - FIR lowering + JIT compilation + callable Rust handles
- `cranelift-ffi`
  - C ABI export and opaque pointers
  - factory cache/lifecycle
  - C++ wrapper classes (`cranelift_dsp`, `cranelift_dsp_factory`)
  - UI/meta callback glue (`UIGlue`, `MetaGlue` parity-style adapters)

Factory compilation path (planned):

`createCraneliftDSPFactoryFromFile/String`
-> `compiler` facade (signals/FIR pipeline)
-> Cranelift backend compile/JIT module
-> exported factory object

Important v1 constraint:
- no duplicate compile pipelines in FFI crate; reuse `compiler` facade and backend APIs.
- implement the same cache/factory lifecycle strategy as `llvm_dsp` /
  `interpreter_dsp` exports (including cache lookup/list/delete entry points)

---

## 8. Diagnostics and Error Model

Backend errors should be structured and compatible with the existing compiler
error flow.

## 8.1 Error categories (proposed)

- `UnsupportedNode`
- `UnsupportedType`
- `UnsupportedIntrinsic`
- `InvalidModuleShape`
- `SignatureMismatch`
- `LayoutError`
- `JitInitializationFailed`
- `CodegenFailed` (Cranelift API error)
- `ExecutionFailed` (runtime wrapper/trampoline issues)

## 8.2 Integration requirements

- `Display` + `std::error::Error`
- stable machine-readable code strings
- conversion path to `compiler::CompilerError` (when CLI/API integration is added)

## 8.3 `.clif` persistence contract (save/read)

This section defines how Cranelift factories are persisted and restored.

### 8.3.1 API contract (locked)

The existing C API family names remain unchanged for compatibility:

- `readCCraneliftDSPFactoryFromBitcode`
- `writeCCraneliftDSPFactoryToBitcode`
- `readCCraneliftDSPFactoryFromBitcodeFile`
- `writeCCraneliftDSPFactoryToBitcodeFile`

For the Cranelift backend, these four functions must read/write a `.clif`
container payload (not the current source-backed scaffold format).

### 8.3.2 Container format (`FAUST_CLIF_V1`)

Use a versioned container with deterministic section ordering:

- magic: `FAUST_CLIF_V1`
- header fields:
  - `format_version`
  - `faust_rs_version_or_commit`
  - `cranelift_codegen_version`
  - `target_triple`
  - `pointer_width`
  - `endianness`
  - `opt_level`
  - `compile_options_hash`
  - `dsp_sha`
- sections:
  - `module_metadata` (factory name, io counts, layout digest)
  - `clif_text` (per-function CLIF textual IR)
  - `ffi_contract` (mandatory lifecycle symbol names + signatures)
  - `source_fallback` (optional DSP source to rebuild when CLIF import is rejected)
  - `checksum` (payload integrity)

### 8.3.3 Write path

1. Compile through current compiler/backend path.
2. Capture CLIF textual IR for each exported/generated function.
3. Capture factory metadata required by runtime and FFI instance creation.
4. Serialize deterministic container bytes.
5. Expose bytes via `write*Bitcode` and file output via `write*BitcodeFile`.

Hard failures on write:

- missing mandatory lifecycle symbols (`compute`, init/reset family required by wrapper contract)
- empty function set
- inconsistent metadata/layout information

### 8.3.4 Read path

1. Parse and validate magic/version.
2. Validate compatibility (`target_triple`, pointer width, endianness, supported format).
3. Rebuild module from `clif_text` sections.
4. Rehydrate factory metadata + runtime layout contract.
5. Verify mandatory symbol signatures before returning factory.
6. If direct import is rejected and `source_fallback` exists:
   - rebuild factory from source through normal compiler path
   - tag diagnostic as fallback rebuild (not direct CLIF load)
7. Otherwise return typed load error.

### 8.3.5 Compatibility policy

- Strict major version match (`FAUST_CLIF_V1` only in V1).
- Allow additive sections only if parser supports unknown-section skipping.
- Default behavior is fail-fast on incompatible target metadata.
- `.clif` artifacts are treated as toolchain-coupled unless fallback rebuild is used.

Suggested stable error codes:

- `FRS-CLIF-LOAD-UNSUPPORTED-VERSION`
- `FRS-CLIF-LOAD-INCOMPATIBLE-TARGET`
- `FRS-CLIF-LOAD-VERIFY-FAILED`
- `FRS-CLIF-LOAD-NO-FALLBACK`
- `FRS-CLIF-SAVE-MISSING-SYMBOL`

### 8.3.6 Required tests

1. In-memory round-trip: compile -> write bytes -> read bytes -> create instance -> compute.
2. File round-trip: compile -> write file -> read file -> compute.
3. Corruption tests: bad magic, truncation, checksum mismatch, missing mandatory section.
4. Compatibility tests: incompatible target metadata rejected deterministically.
5. Fallback tests: incompatible CLIF + valid source fallback rebuilds and runs.
6. Determinism tests: repeated writes are stable (excluding explicitly non-deterministic fields).

### 8.3.7 Rollout sequence

1. Add internal serializer/deserializer module for `FAUST_CLIF_V1`.
2. Wire `read*/write*Bitcode[File]` to `.clif` path.
3. Keep legacy scaffold parser only for short migration window in tests (if needed).
4. Remove scaffold format support, update headers/docs/JOURNAL.

---

## 9. Validation Strategy (Systematic Alignment)

This backend must be introduced with systematic validation to avoid silent drift.

## 9.1 Validation layers

1. **FIR verifier gate**
   - run `verify_fir_module` before Cranelift lowering in tests and validation tools
2. **Backend unit tests**
   - type mapping, signature mapping, variable layout, lowering of loops/branches
3. **Backend execution tests**
   - compile and run minimal FIR modules directly (without full parser pipeline)
4. **FFI runtime tests (C ABI and C++ wrapper layer)**
   - factory creation from file/string
   - instance lifecycle and `compute`
  - `buildUserInterface` callback dispatch
  - `metadata` callback dispatch
   - `add_soundfile` UI callback dispatch (at least passthrough callback invocation)
   - multiple instances per factory
5. **End-to-end differential tests**
   - same DSP through `interp` and `cranelift`, compare traces

## 9.2 Differential oracle (v1)

Use the existing Rust interpreter backend as the primary runtime oracle:

- compare `interp` outputs vs `cranelift` outputs
- same scenarios and tolerances as runtime trace validation harness

Known limitation:
- shared pipeline bugs may affect both; mitigate with existing golden checks and
  selected C++ parity checks.

## 9.3 `xtask` integration plan

### v1

- add `xtask` command(s) for Cranelift trace dump/check (or extend existing trace harness with `--backend interp|cranelift`)
- add smoke command(s) for `cranelift-ffi` factory/instance lifecycle checks
- add smoke checks for UI/meta callback paths (`buildUserInterface`, `metadata`)

### v1.1

- extend `backend-align-smoke` / `backend-align-nightly` to include Cranelift
  runtime checks on a small curated subset

## 9.4 Initial smoke corpus recommendation

Start with runtime cases already used in `interp` smoke validation:

- `trace_01_passthrough`
- `trace_07_nonlinear_clip`
- `trace_38_sine_phasor`

Then expand to:

- `trace_03_stereo_mix`
- `trace_09_ui_slider` (if lifecycle/control support is sufficient)
- `trace_31_extended_primitives_typed`

---

## 10. Finish Backlog from Current State (Execution Order)

This section is the concrete completion plan from the current codebase state.
Each step is intentionally commit-sized and has strict pass criteria.

Execution status snapshot (2026-02-27):

- Step A1: `done`
- Step A2: `done`
- Step B1: `done` (signals/boxes constructors implemented through `box-ffi` handle bridge)
- Step B2: `pending` (current implementation is source-backed rebuild; migrate to `.clif` contract from Section 8.3)
- Step C1: `done`
- Step C2: `done` (strict subset mode + validation gate wiring)
- Step D1: `adapted` (runtime differential smoke implemented for `rep_01`/`rep_07` + UI/meta callback smoke; `rep_38` remains deferred)
- Step D2: `adapted` (Cranelift strict/runtime gates are wired into `backend-align-smoke/nightly`; full command success still depends on existing interp trace snapshot drift unrelated to Cranelift)
- Step E1: `done`

### Step A1 — Real Cranelift instance runtime in `cranelift-ffi`

Scope:

- Replace scaffold no-op behavior in:
  - `computeCCraneliftDSPInstance`
  - `buildUserInterfaceCCraneliftDSPInstance`
  - `metadataCCraneliftDSPInstance`
- Wire instance state allocation to the backend `dsp*` layout
  (`JitDspModule::struct_layout()`), so JIT compute receives a valid `dsp` pointer.
- Isolate all required function-pointer invocation `unsafe` with Rustdoc `# Safety`
  invariants and dedicated tests.

Pass criteria:

- C API smoke test executes non-trivial audio (`count > 0`) and mutates outputs.
- `metadata` and `buildUserInterface` dispatch real callback events (not placeholder strings only).
- `cargo test -p cranelift-ffi` passes with no scaffold-only compute path.

### Step A2 — Remove scaffold semantics in factory/instance metadata

Scope:

- Remove scaffold markers from JSON/status fields where runtime is now real.
- Fill `num_inputs` / `num_outputs` from real FIR/module metadata.
- Keep remaining deferred families explicitly labeled as deferred.

Pass criteria:

- Factory and instance tests assert real backend state fields.
- No remaining `"status":"scaffold"` dependency in runtime correctness tests.

### Step B1 — Complete mandatory FFI factory families

Scope:

- Implement `createCCraneliftDSPFactoryFromSignals`.
- Implement `createCCraneliftDSPFactoryFromBoxes`.
- Reuse compiler facade and existing conversion path (no duplicate pipelines).

Pass criteria:

- File/string/signals/boxes constructors all compile the same DSP semantics for smoke fixtures.
- Cache-key behavior consistent across constructor families for identical resulting DSP/options.

### Step B2 — Replace scaffold bitcode family

Scope:

- Replace temporary scaffold text serializer with a real, documented backend
  persistence contract for Cranelift factories based on `.clif` payloads.
- Keep function family names unchanged (`read*/write*Bitcode[File]`) for API
  parity, but map those functions to `.clif` read/write semantics.

Pass criteria:

- `.clif` in-memory and file round-trip produce runnable factories.
- Round-trip preserves compile options, factory identity fields, and lifecycle
  metadata needed by instance creation.

### Step C1 — Lowering coverage closure on corpus

Scope:

- Run corpus diagnostics with `diagnose_cranelift_compute_subset_gap`.
- Prioritize and implement missing FIR nodes/intrinsics required by selected
  runtime corpus (`tests/corpus` + runtime traces subset).
- Keep pre-check and lowering in lockstep.

Pass criteria:

- Selected v1 corpus compiles with `compute_body_lowered == true` (no stub fallback).
- Any remaining unsupported nodes are explicitly documented in this plan and `JOURNAL`.

### Step C2 — Fallback policy hardening

Scope:

- Add a strict mode that turns subset fallback into hard error.
- Use strict mode in validation/CI gates.

Pass criteria:

- CI validation path fails when any selected case falls back to stub.
- Developer mode can still optionally keep permissive behavior for exploratory scans.

### Step D1 — Differential runtime validation (`interp` vs `cranelift`)

Scope:

- Add/extend automated runtime differential checks with numeric tolerance.
- Cover at least the initial smoke corpus plus UI/meta callback smoke paths.

Pass criteria:

- Repeatable one-command local run for Cranelift differential checks.
- Failure reports include case name, backend, and first mismatch context.

### Step D2 — `xtask` + CI integration

Scope:

- Add Cranelift backend checks to PR-level smoke workflow (small subset).
- Keep extended subset/nightly separately if runtime cost is high.

Pass criteria:

- CI has at least one mandatory Cranelift runtime smoke gate.
- Gate includes FFI lifecycle + compute + UI/meta callback coverage.

### Step E1 — Documentation and parity matrix closure

Scope:

- Update `porting/cranelift-dsp-ffi-parity-matrix-en.md` after each implemented family.
- Keep explicit status per family: `done`, `adapted`, `deferred-v1`.
- Document every remaining deferred item with rationale and owner.

Pass criteria:

- Matrix matches actual exported symbols and behavior.
- No undocumented divergence between plan and code.

### Immediate next PR sequence (recommended)

1. PR-1: Step B2 (`.clif` persistence path + round-trip tests).
2. PR-2: Step C1 targeted lowering closure for smoke corpus.
3. PR-3: Step C2 + Step D1 + Step D2 (strict gate + automated validation).

---

## 11. Implementation Phases (Detailed)

Note: this phase decomposition is kept as the baseline roadmap. The current
codebase has already entered implementation beyond pure scaffolding, and the
authoritative completion order from now on is the execution backlog in Section 10.

## Phase 0 — Backend/FFI Design Freeze (2–4 days)

### Deliverables

- Finalized v1 ABI contract
- Finalized exported `cranelift_dsp` C/C++ lifecycle contract (factory + instance)
- Function-by-function export parity matrix against `llvm_dsp` / `interpreter_dsp`
- Cache strategy parity matrix (same exported cache entry points and semantics)
- FIR subset inventory (supported/deferred)
- Cranelift crate selection and feature flag plan
- Error-code namespace draft

### Pass criteria

- Design review notes accepted
- No unresolved blockers on ownership/lifetime model or JIT/FFI lifetime coupling

## Phase 1 — Scaffolding and Feature-Gated Modules (2–4 days)

### Deliverables

- `codegen::backends::cranelift` module skeleton
- feature flag wiring in `codegen` (and optional `compiler`)
- `CraneliftOptions`, error types, placeholder entry points
- `cranelift-ffi` crate skeleton (`factory.rs`, `instance.rs`, `types.rs`, headers placeholders)
- placeholder C/C++ headers listing the target export set (parity matrix reflected in code)

### Pass criteria

- `cargo check` passes with feature off and on
- unsupported placeholder path returns structured error (no panic)
- FFI crate builds and exports placeholder symbols without runtime implementation

## Phase 2 — Type/Signature/Layout Mapping (3–5 days)

### Deliverables

- FIR type -> Cranelift type mapper (v1 subset)
- function signature builder for `compute`
- DSP struct field layout planner (`kStruct` offsets + alignment policy)
- FFI-visible instance/factory layout ownership model (opaque wrappers only; no ABI exposure of Rust internals)

### Pass criteria

- unit tests for:
  - scalar types
  - pointer args
  - `compute` signature shape
  - deterministic field offsets/layout
  - C/C++ wrapper-callable function pointer signature compatibility

## Phase 3 — Value + Statement Lowering Core (5–10 days)

### Deliverables

- lowering of v1 value subset
- lowering of v1 statement subset
- loop lowering for `SimpleForLoop` and `ForLoop`
- branch lowering (`If`, `Select2`)
- lowering of FIR UI/meta statements to callback trampolines in generated functions

### Pass criteria

- backend unit tests cover:
  - arithmetic
  - casts
  - loops
  - input/output buffer indexing
  - UI/meta statement lowering dispatch
- no backend panics on unsupported nodes (typed errors only)

## Phase 4 — JIT Runtime Wrapper and Executable `compute` (4–8 days)

### Deliverables

- `JitDspModule` + `JitDspInstance`
- machine code finalization and callable `compute`
- safe Rust wrapper around raw function pointers (unsafe isolated and documented)
- exported internal hooks sufficient for FFI layer integration

### Pass criteria

- executes hand-built FIR test modules correctly
- multiple instances from one compiled module are supported
- explicit safety docs on function pointer transmute/trampoline boundaries

## Phase 5 — End-to-End Compiler Integration + `cranelift_dsp` FFI Export (5–10 days)

### Deliverables

- compile DSP -> FIR -> Cranelift JIT path exposed through Rust test helpers
- `compiler` helper API for Cranelift compile-and-run (Rust-side)
- `cranelift-ffi` factory creation from file/string via `compiler` facade
- `cranelift-ffi` cache API/export set parity implementation (`llvm_dsp` / `interpreter_dsp` strategy)
- C API instance lifecycle + `compute`
- C API `buildUserInterface` + `metadata` callback dispatch
- C++ wrapper (`cranelift-dsp.h`) basic class surface (`cranelift_dsp`, factory wrapper)

### Pass criteria

- selected runtime corpus smoke subset runs end-to-end
- differential traces vs `interp` pass within tolerances on supported cases
- C and C++ smoke tests can instantiate a `cranelift_dsp` and call `compute`
- C and C++ smoke tests can call `buildUserInterface` and `metadata` with recorder callbacks
- C API cache operations behave consistently with reference runtime families

## Phase 6 — Validation Tooling (`xtask`) and CI Hook (4–7 days)

### Deliverables

- Cranelift runtime trace check command (new or extended existing command)
- Cranelift FFI smoke validation command (or CI test target) for C/C++ exported path
- expected-skip classification for unsupported cases
- integration into `backend-align-smoke` (PR subset) behind a flag or default once stable

### Pass criteria

- local repeatable validation command
- CI runtime acceptable (documented)
- failures report backend/lane/case/scenario clearly
- exported C/C++ path covered by at least one automated CI smoke check
- exported UI/meta callback path covered by at least one automated smoke check

## Phase 7 — Coverage Expansion and Hardening (ongoing)

### Deliverables

- broader FIR node support (`IteratorForLoop`, `Switch`, more intrinsics)
- lifecycle/UI/metadata coverage expansion beyond the V1 mandatory subset
- AOT object emission prototype (`cranelift-object`)

### Pass criteria

- expanded differential coverage
- documented support matrix and remaining gaps

---

## 12. Test Strategy (Detailed)

## 12.1 Unit tests (`codegen::backends::cranelift`)

- type mapping
- signature mapping
- intrinsic mapping table
- struct layout offsets and alignment
- lowering of representative FIR fragments
- unsupported FIR node/type diagnostics

## 12.2 Integration tests (new crate tests or `compiler` tests)

- compile selected DSPs to FIR, run via Cranelift JIT
- compare output buffers to interpreter traces within tolerance

## 12.3 FFI/export tests (`cranelift-ffi`)

- C API:
  - export parity smoke (expected function set present and callable)
  - create factory from string/file
  - create/delete instance
  - `init` + `compute`
  - `buildUserInterface` with `UIGlue` callback recorder
  - `metadata` with `MetaGlue` callback recorder
  - `add_soundfile` callback invocation behavior (smoke/contract test)
  - invalid pointer/null pointer behavior
  - repeated factory/instance lifecycle and cache behavior
  - cache API semantics parity checks (lookup/list/delete/all delete)
- C++ wrapper:
  - basic compile + instantiate + compute smoke
  - `buildUserInterface` / `metadata` callback smoke
  - lifecycle order consistency vs wrapper expectations

## 12.4 Differential regression policy

For every newly supported FIR family (example: `Switch`):

1. add unit test for lowering shape
2. add end-to-end DSP test exercising the feature
3. add/enable runtime trace differential case if stable

---

## 13. Performance and Benchmarking Plan

Performance is secondary to correctness for v1, but basic measurement is still required.

## 13.1 Metrics (v1)

- FIR -> Cranelift compile time
- JIT finalize time
- runtime throughput vs `interp` on selected cases

## 13.2 Benchmark corpus (initial)

- `trace_01_passthrough`
- `trace_07_nonlinear_clip`
- `trace_31_extended_primitives_typed`
- `trace_38_sine_phasor`

## 13.3 Acceptance (v1, non-blocking)

- no hard performance target yet
- benchmark results documented in `JOURNAL.md` before broad CI enablement

---

## 14. Risks and Mitigations

## 14.1 Key risks

1. **ABI/layout mismatch for `kStruct`/buffers**
   - Mitigation: centralize layout planner + unit tests + differential runtime checks
2. **Intrinsic coverage gaps (math functions)**
   - Mitigation: curated allowlist, typed unsupported errors, corpus staging
3. **Cranelift type/legalization constraints**
   - Mitigation: start with narrow FIR subset; add explicit type normalization layer
4. **Unsafe boundary bugs in JIT trampoline**
   - Mitigation: isolate unsafe code, document invariants, add dedicated tests
5. **FFI/export lifecycle divergence from `llvm_dsp` / `interpreter_dsp` expectations**
   - Mitigation: explicit lifecycle contract tests (C + C++) and wrapper smoke tests
6. **UI/meta callback ABI mismatch (function pointers / userdata / signatures)**
   - Mitigation: dedicated callback recorder tests (C + C++) and documented trampoline invariants
7. **Backend/tooling drift**
   - Mitigation: integrate into `xtask backend-align-*` workflow early
8. **Cache strategy divergence from reference runtime families**
   - Mitigation: parity matrix + cache API semantics tests + shared FFI design review

## 14.2 Existing project risks to re-check (from critical points note)

This backend does not replace the project’s main critical risks, but it can be
impacted by:

- hidden coupling in active compile flow (`gGlobal` decomposition assumptions)
- choosing the wrong signal->FIR path (must validate lane coverage explicitly)
- compiler-to-Wasm constraints (if sharing abstractions later)

Reference: `porting/faust-rust-points-critiques-en.md`

---

## 15. Documentation and Provenance Requirements

For implementation PRs touching the Cranelift backend or `cranelift-ffi`:

- add Rustdoc provenance/comments for backend invariants and layout rules
- document any `unsafe` block with:
  - why it is needed
  - invariants required
  - how tests validate it
- record public API mapping status (`adapted`/`deferred`) and compatibility notes
  in `JOURNAL.md` or this plan as scope evolves

## 15.1 Collaboration requirement during porting (mandatory)

During implementation/porting phases, if any ambiguity, missing requirement, or
design trade-off is encountered that is not already resolved by this plan, the
implementer must **ask the requester immediately before proceeding** with the
affected part.

This requirement is especially mandatory for:

- C/C++ export parity decisions (`llvm_dsp` / `interpreter_dsp` behavior)
- cache semantics and lifecycle behavior
- ABI/layout/calling-convention choices
- UI/meta callback behavior and soundfile callback semantics
- unsupported FIR handling policy when parity expectations are unclear

Rule of execution:

- do not silently choose a behavior in these areas when the plan or existing
  code is ambiguous
- stop, list the concrete question(s), and get confirmation before continuing

---

## 16. Exit Criteria for v1

The Cranelift backend v1 is considered achieved when all of the following are true:

1. Feature-gated Cranelift backend compiles on CI-supported host platforms (at least Linux/macOS baseline).
2. `compute` runs correctly for a documented smoke corpus subset.
3. Differential runtime traces vs `interp` pass on that subset.
4. Exported C API (`cranelift-dsp-c.h`) can create factory/instance and run `compute` on a smoke case.
5. Exported C API supports `buildUserInterface` and `metadata` callback dispatch on smoke cases.
6. Exported C++ wrapper API (`cranelift-dsp.h`) provides `cranelift_dsp`-style smoke execution on a smoke case.
7. Exported C++ wrapper API supports `buildUserInterface` and `metadata` callback dispatch on smoke cases.
8. Exported C API/C++ wrapper expose the target parity function set (including cache management functions) matching `llvm_dsp` / `interpreter_dsp` strategy.
9. Unsupported FIR coverage is explicit and reported via typed errors (no silent miscompilation).
10. Backend + exported path are integrated into at least one repeatable validation command/test flow (`xtask` and/or CI smoke).
11. Documentation exists for ABI/layout, unsafe boundaries, exported lifecycle contract, cache parity contract, UI/meta callback contract, and known gaps.

---

## 17. Recommended Next Step (Implementation kickoff)

Start with **Phase 0 + Phase 1 only** in the first implementation PR:

- backend scaffolding
- feature flags
- options/errors
- design checks for ABI + FIR subset

Do **not** start full lowering before the v1 internal ABI, exported `cranelift_dsp`
contract, and support subset are frozen in code review.
