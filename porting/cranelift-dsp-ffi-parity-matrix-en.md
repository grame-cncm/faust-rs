# Cranelift DSP FFI Parity Matrix (Phase 0)

**Date:** 2026-02-27  
**Status:** Active implementation tracking (E1 closure update)  
**Primary reference:** `llvm_dsp` / `llvm_dsp_factory` (C + C++)  
**Secondary cross-check:** `interpreter_dsp` / `interpreter_dsp_factory` (C + C++)  
**Target surface:** `cranelift_dsp` / `cranelift_dsp_factory` (C + C++)

---

## 1. Purpose

This document is the **Phase 0 function-by-function parity matrix** required by:

- `porting/cranelift-backend-plan-en.md`

It defines the target exported function set and naming strategy for the future
`cranelift_dsp` FFI layer before implementation deepens.

---

## 2. Locked Decisions

## 2.1 Reference priority

- Primary reference: `llvm_dsp`
- Secondary cross-check: `interpreter_dsp`
- If they diverge:
  - default to `llvm_dsp`
  - document the divergence and rationale before implementation

## 2.2 C API naming convention

Use the **interpreter-style backend-prefixed** naming for Cranelift C functions.

Examples:

- `createCCraneliftDSPFactoryFromFile`
- `createCCraneliftDSPFactoryFromString`
- `createCCraneliftDSPInstance`

Rule:

- Preserve the **function family/semantics** parity of `llvm_dsp`
- Replace the backend-specific naming prefix with `Cranelift`
- For LLVM-only generic helper/query C symbols, use the same
  interpreter-style backend-prefixed approach (no LLVM generic symbol reuse as
  the primary Cranelift name).

## 2.3 Current implementation closure status (2026-02-27)

Status legend used for implementation tracking:

- `done`: implemented and validated in current codebase
- `adapted`: implemented with documented behavior differences vs LLVM reference
- `deferred-v1`: intentionally not exported/implemented in V1

| Family | Status | Notes |
|---|---|---|
| Global shared (`getCLibFaustVersion`, `startMTDSPFactories`, `stopMTDSPFactories`, `freeCMemory`) | `done` | Exported and tested in `cranelift-ffi`. |
| Factory cache lifecycle (`get*FromSHAKey`, `deleteAll*`, `getAll*`) | `done` | Shared cache wired with SHA map and deletion APIs. |
| Factory constructors from file/string | `done` | Fast-lane compiler path + Cranelift JIT + interp sidecar for UI/meta. |
| Factory constructors from signals/boxes | `adapted` | Implemented via `box-ffi` handle bridge (`CboxesToSignals*` contract). |
| Factory metadata/query surface (`JSON`, `name`, `sha`, `DSPCode`, `compile options`) | `done` | Exported and covered by runtime tests. |
| Factory library/include/warning lists | `adapted` | Exported with empty-list behavior for current V1 runtime. |
| Bitcode read/write family | `adapted` | Source-backed `CRANELIFT_FFI_V2_SOURCE` payload with rebuild; non-source factories (signals/boxes) return null/false on write. |
| IR read/write family | `deferred-v1` | Not exported in V1. |
| Machine/object read/write family | `deferred-v1` | Not exported in V1. |
| Memory manager / foreign-function registration | `adapted` | Foreign registration is implemented; `-mem0` uses the shared aligned ABI v1 through `setCCraneliftMemoryManager`, while the C++ wrapper supplies functional legacy set/get glue. |
| Target/machine-target getter family | `deferred-v1` | LLVM-specific semantics intentionally deferred. |
| DSP instance lifecycle (`create/delete/clone/init/reset/clear`) | `done` | Real instance state allocation + lifecycle hooks. |
| DSP instance compute | `done` | Calls finalized Cranelift JIT entry with backend `dsp*` layout. |
| DSP instance UI/meta callbacks | `adapted` | Implemented via interpreter sidecar instruction dispatch; metadata includes Cranelift-specific diagnostics keys. |
| Runtime differential validation (`interp` vs `cranelift`) | `adapted` | Automated smoke differential tests currently cover `rep_01` and `rep_07`; `rep_38` remains tracked for later parity completion. |
| CI/PR smoke gate integration | `adapted` | `backend-align-smoke/nightly` now execute strict Cranelift subset gate + Cranelift runtime diff smoke before interp trace checks. |

---

## 3. C API Parity Matrix (Phase 0 Working Draft)

Legend:

- `Target`: Cranelift C API target symbol name
- `Status`: `v1-required`, `v1-deferred`, `n/a`, `decision-pending`
- `Ref`: primary/secondary reference note

## 3.1 Global / shared functions

| Family | LLVM C API | Interpreter C API | Target (Cranelift C API) | Status | Notes |
|---|---|---|---|---|---|
| Version | `getCLibFaustVersion` | `getCLibFaustVersion` | `getCLibFaustVersion` | `v1-required` | Shared global symbol (same name) |
| MT start | `startMTDSPFactories` | `startMTDSPFactories` | `startMTDSPFactories` | `v1-required` | Shared cache/threading strategy |
| MT stop | `stopMTDSPFactories` | `stopMTDSPFactories` | `stopMTDSPFactories` | `v1-required` | Shared cache/threading strategy |

## 3.2 Factory cache lifecycle

| Family | LLVM C API | Interpreter C API | Target (Cranelift C API) | Status | Notes |
|---|---|---|---|---|---|
| Cache get by SHA | `getCDSPFactoryFromSHAKey` | `getCInterpreterDSPFactoryFromSHAKey` | `getCCraneliftDSPFactoryFromSHAKey` | `v1-required` | Backend-prefixed naming, cache parity required |
| Delete all factories | `deleteAllCDSPFactories` | `deleteAllCInterpreterDSPFactories` | `deleteAllCCraneliftDSPFactories` | `v1-required` | |
| List all factories | `getAllCDSPFactories` | `getAllCInterpreterDSPFactories` | `getAllCCraneliftDSPFactories` | `v1-required` | Array/freeing semantics parity |

## 3.3 Factory creation from Faust source

| Family | LLVM C API | Interpreter C API | Target (Cranelift C API) | Status | Notes |
|---|---|---|---|---|---|
| From file | `createCDSPFactoryFromFile` | `createCInterpreterDSPFactoryFromFile` | `createCCraneliftDSPFactoryFromFile` | `v1-required` | User-locked naming; LLVM adds `(target, opt_level)` args |
| From string | `createCDSPFactoryFromString` | `createCInterpreterDSPFactoryFromString` | `createCCraneliftDSPFactoryFromString` | `v1-required` | LLVM adds `(target, opt_level)` args |
| From signals | `createCDSPFactoryFromSignals` | `createCInterpreterDSPFactoryFromSignals` | `createCCraneliftDSPFactoryFromSignals` | `v1-required` | LLVM adds `(target, opt_level)` args; may return typed unsupported initially |
| From boxes | `createCDSPFactoryFromBoxes` | `createCInterpreterDSPFactoryFromBoxes` | `createCCraneliftDSPFactoryFromBoxes` | `v1-required` | LLVM adds `(target, opt_level)` args; may return typed unsupported initially |

### 3.3.1 Signature delta note (LLVM vs Interpreter)

LLVM source-creation APIs include backend-specific compilation parameters:

- `const char* target`
- `int opt_level`

Interpreter source-creation APIs do not.

Phase-0 implication for `cranelift_dsp`:

- Keep the **same exported family** as LLVM/interpreter parity matrix requires.
- User-locked source-creation signature policy:
  - keep `opt_level` (when Cranelift optimization levels are exposed)
  - drop LLVM-specific `target` string parameter
- This is an **adapted ABI shape** relative to `llvm_dsp` source-creation C APIs.
  Function-family parity is preserved; signature parity is intentionally adapted.

## 3.4 Factory deletion and metadata/query surface

| Family | LLVM C API | Interpreter C API | Target (Cranelift C API) | Status | Notes |
|---|---|---|---|---|---|
| Delete factory | `deleteCDSPFactory` | `deleteCInterpreterDSPFactory` | `deleteCCraneliftDSPFactory` | `v1-required` | |
| JSON | `getCDSPFactoryJSON` | `getCInterpreterDSPFactoryJSON` | `getCCraneliftDSPFactoryJSON` | `v1-required` | |
| Library list | `getCDSPFactoryLibraryList` | `getCInterpreterDSPFactoryLibraryList` | `getCCraneliftDSPFactoryLibraryList` | `v1-required` | |
| Name | `getCName` | C++ method only helper equivalent | `getCCraneliftDSPFactoryName` | `v1-required` | Naming locked to interpreter-style backend prefix |
| SHA key | `getCSHAKey` | no same C symbol in interpreter C header | `getCCraneliftDSPFactorySHAKey` | `v1-required` | Naming locked to interpreter-style backend prefix |
| Expanded code | `getCDSPCode` | not present in interpreter C header | `getCCraneliftDSPFactoryDSPCode` | `v1-required` | Naming locked; implementation may initially return typed unsupported only if necessary |
| Compile options | `getCDSPFactoryCompileOptions` | not present in interpreter C header | `getCCraneliftDSPFactoryCompileOptions` | `v1-required` | Naming locked to interpreter-style backend prefix |
| Target string | `getCTarget` | not present | `getCCraneliftDSPFactoryTarget` | `v1-deferred` | Naming locked; deferred in V1 (target semantics are LLVM-specific) |
| Include pathnames | `getCDSPFactoryIncludePathnames` | not present in interpreter C header | `getCCraneliftDSPFactoryIncludePathnames` | `v1-required` | Naming locked to interpreter-style backend prefix |
| Warning messages | `getCWarningMessages` | not present in interpreter C header | `getCCraneliftDSPFactoryWarningMessages` | `v1-required` | Naming locked to interpreter-style backend prefix |

## 3.5 Factory serialization / code import-export families

| Family | LLVM C API | Interpreter C API | Target (Cranelift C API) | Status | Notes |
|---|---|---|---|---|---|
| Bitcode read/write (backend bytecode) | `readCDSPFactoryFromBitcode`, `writeCDSPFactoryToBitcode`, `...File` | `readCInterpreterDSPFactoryFromBitcode`, `writeCInterpreterDSPFactoryToBitcode`, `...File` | `readCCraneliftDSPFactoryFromBitcode`, `writeCCraneliftDSPFactoryToBitcode`, `...File` | `v1-required` | Exact backend payload format TBD (likely Cranelift-specific compiled snapshot or deferred stub with typed error) |
| IR read/write | `readCDSPFactoryFromIR`, `writeCDSPFactoryToIR`, `...File` | n/a | `readCCraneliftDSPFactoryFromIR`, `writeCCraneliftDSPFactoryToIR`, `...File` | `v1-deferred` | Deferred in V1 **without exported symbols** |
| Machine read/write | `readCDSPFactoryFromMachine`, `writeCDSPFactoryToMachine`, `...File` | n/a | `readCCraneliftDSPFactoryFromMachine`, `writeCCraneliftDSPFactoryToMachine`, `...File` | `v1-deferred` | Deferred in V1 **without exported symbols** |

## 3.6 Factory runtime/global configuration

| Family | LLVM C API | Interpreter C API | Target (Cranelift C API) | Status | Notes |
|---|---|---|---|---|---|
| Class init on factory | `classCInit` | n/a | `classCCraneliftInit` (naming pending) | `decision-pending` | LLVM-specific pattern; may be represented via instance APIs only |
| Memory manager setter | `setCMemoryManager` | n/a (in current interpreter C header) | `setCCraneliftMemoryManager` | `adapted` | Implemented for factories compiled with `-mem0`; copies ABI v1 callbacks/context, describes immediately, and allocates only on instance creation. |
| Register foreign function | `registerCForeignFunction` | n/a | `registerCCraneliftForeignFunction` | `v1-deferred` | Naming locked; family deferred in V1 |
| Machine target getter | `getCDSPMachineTarget` | n/a | `getCCraneliftDSPMachineTarget` | `v1-deferred` | Naming locked; deferred in V1 |

## 3.7 DSP instance functions (C API)

| Family | LLVM C API | Interpreter C API | Target (Cranelift C API) | Status | Notes |
|---|---|---|---|---|---|
| Create instance | `createCDSPInstance` | `createCInterpreterDSPInstance` | `createCCraneliftDSPInstance` | `v1-required` | User-locked naming |
| Delete instance | `deleteCDSPInstance` | `deleteCInterpreterDSPInstance` | `deleteCCraneliftDSPInstance` | `v1-required` | |
| Clone | `cloneCDSPInstance` | `cloneCInterpreterDSPInstance` | `cloneCCraneliftDSPInstance` | `v1-required` | May return typed unsupported initially only if absolutely necessary (prefer implement) |
| Num inputs | `getNumInputsCDSPInstance` | `getNumInputsCInterpreterDSPInstance` | `getNumInputsCCraneliftDSPInstance` | `v1-required` | |
| Num outputs | `getNumOutputsCDSPInstance` | `getNumOutputsCInterpreterDSPInstance` | `getNumOutputsCCraneliftDSPInstance` | `v1-required` | |
| Build UI | `buildUserInterfaceCDSPInstance` | `buildUserInterfaceCInterpreterDSPInstance` | `buildUserInterfaceCCraneliftDSPInstance` | `v1-required` | UI callbacks mandatory in V1 |
| Sample rate | `getSampleRateCDSPInstance` | `getSampleRateCInterpreterDSPInstance` | `getSampleRateCCraneliftDSPInstance` | `v1-required` | |
| Init | `initCDSPInstance` | `initCInterpreterDSPInstance` | `initCCraneliftDSPInstance` | `v1-required` | |
| instanceInit | `instanceInitCDSPInstance` | `instanceInitCInterpreterDSPInstance` | `instanceInitCCraneliftDSPInstance` | `v1-required` | |
| instanceConstants | `instanceConstantsCDSPInstance` | `instanceConstantsCInterpreterDSPInstance` | `instanceConstantsCCraneliftDSPInstance` | `v1-required` | |
| instanceResetUserInterface | `instanceResetUserInterfaceCDSPInstance` | `instanceResetUserInterfaceCInterpreterDSPInstance` | `instanceResetUserInterfaceCCraneliftDSPInstance` | `v1-required` | |
| instanceClear | `instanceClearCDSPInstance` | `instanceClearCInterpreterDSPInstance` | `instanceClearCCraneliftDSPInstance` | `v1-required` | |
| Metadata | `metadataCDSPInstance` | `metadataCInterpreterDSPInstance` | `metadataCCraneliftDSPInstance` | `v1-required` | Meta callbacks mandatory in V1 |
| Compute | `computeCDSPInstance` | `computeCInterpreterDSPInstance` | `computeCCraneliftDSPInstance` | `v1-required` | |

## 3.8 Memory helpers / shared utilities

| Family | LLVM C API | Interpreter C API | Target (Cranelift C API) | Status | Notes |
|---|---|---|---|---|---|
| `freeCMemory` | Present in LLVM C API family support stack | Present in interpreter C API implementation/export | `freeCMemory` | `v1-required` | Shared utility name (same symbol strategy) |

## 3.9 C API exact-signature inventory (to finalize before ABI freeze)

This section tracks the concrete signature families that must be frozen before
implementing the exported `cranelift_dsp` ABI. It is intentionally focused on
the **shape** differences that matter for parity and compatibility.

### 3.9.1 Global/shared (common signatures)

- `const char* getCLibFaustVersion(void);`
- `bool startMTDSPFactories(void);`
- `void stopMTDSPFactories(void);`
- `void freeCMemory(void* ptr);` (shared utility, declared in support headers)

### 3.9.2 Factory creation (LLVM-style extended shape vs Interpreter reduced shape)

LLVM C API shape examples:

- `createCDSPFactoryFromFile(filename, argc, argv, target, error_msg, opt_level)`
- `createCDSPFactoryFromString(name_app, dsp_content, argc, argv, target, error_msg, opt_level)`
- `createCDSPFactoryFromSignals(name_app, signals, argc, argv, target, error_msg, opt_level)`
- `createCDSPFactoryFromBoxes(name_app, box, argc, argv, target, error_msg, opt_level)`

Interpreter C API shape examples:

- `createCInterpreterDSPFactoryFromFile(filename, argc, argv, error_msg)`
- `createCInterpreterDSPFactoryFromString(name_app, dsp_content, argc, argv, error_msg)`
- `createCInterpreterDSPFactoryFromSignals(name_app, signals, argc, argv, error_msg)`
- `createCInterpreterDSPFactoryFromBoxes(name_app, box, argc, argv, error_msg)`

Cranelift target:

- naming is locked (`createCCranelift...`)
- source-creation signatures keep `opt_level` and omit LLVM-specific `target`

### 3.9.3 Serialization families (signature group deltas)

LLVM includes target/opt-level parameters in several read functions:

- bitcode read/read-file: `(bit_code, target, error_msg, opt_level)`
- IR read/read-file: `(ir_code/path, target, error_msg, opt_level)`
- machine read/read-file: target-aware forms

Interpreter bitcode forms are simpler:

- `readCInterpreterDSPFactoryFromBitcode(bitcode, error_msg)`
- `readCInterpreterDSPFactoryFromBitcodeFile(bit_code_path, error_msg)`

Cranelift target:

- bitcode family is `v1-required`
- IR/machine families are `v1-deferred` **without exported symbols** in V1

### 3.9.4 Instance methods (C API)

LLVM and Interpreter C instance signatures are effectively aligned in shape:

- `getNumInputs*`, `getNumOutputs*`, `getSampleRate*`
- `buildUserInterface*(..., UIGlue*)`
- `metadata*(..., MetaGlue*)`
- `init*`, `instanceInit*`, `instanceConstants*`, `instanceResetUserInterface*`, `instanceClear*`
- `clone*`
- `compute*(..., int count, FAUSTFLOAT** input, FAUSTFLOAT** output)`
- `create*Instance(factory)`, `delete*Instance(dsp)`

This aligned shape is the default target for Cranelift V1.

---

## 4. C++ Wrapper Parity Matrix (Phase 0 Working Draft)

Phase 0 requirement is to mirror the **usage strategy** and class roles of:

- `llvm_dsp` / `llvm_dsp_factory`
- `interpreter_dsp` / `interpreter_dsp_factory`

Initial V1 target classes:

- `cranelift_dsp` (inherits/behaves as `dsp`)
- `cranelift_dsp_factory` (inherits/behaves as `dsp_factory`)

### 4.1 Wrapper class roles (V1)

- create/delete factory wrappers over C API
- create/delete DSP instances
- `init`, `compute`
- `buildUserInterface`, `metadata`
- cache-aware factory acquisition/release strategy

### 4.2 DSP instance class method parity (`llvm_dsp` vs `interpreter_dsp`)

Observed parity is strong; both classes expose the same operational surface.

| Method family | `llvm_dsp` | `interpreter_dsp` | `cranelift_dsp` target | Status | Notes |
|---|---|---|---|---|---|
| Inputs/outputs | `getNumInputs`, `getNumOutputs` | same | same | `v1-required` | |
| UI build | `buildUserInterface(UI*)` | same | same | `v1-required` | UI mandatory in V1 |
| Sample rate | `getSampleRate` | same | same | `v1-required` | |
| Init family | `init`, `instanceInit`, `instanceConstants`, `instanceResetUserInterface`, `instanceClear` | same | same | `v1-required` | |
| Clone | `clone()` | same | same | `v1-required` | |
| Metadata | `metadata(Meta*)` | same | same | `v1-required` | Meta mandatory in V1 |
| Compute | `compute(int, FAUSTFLOAT**, FAUSTFLOAT**)` | same | same | `v1-required` | |

### 4.3 DSP factory class method parity (`llvm_dsp_factory` vs `interpreter_dsp_factory`)

Most factory methods overlap; LLVM adds target/class-init specific APIs.

| Method family | `llvm_dsp_factory` | `interpreter_dsp_factory` | `cranelift_dsp_factory` target | Status | Notes |
|---|---|---|---|---|---|
| Destructor | `~llvm_dsp_factory()` | `~interpreter_dsp_factory()` | `~cranelift_dsp_factory()` | `v1-required` | |
| Name | `getName()` | `getName()` | `getName()` | `v1-required` | |
| SHA key | `getSHAKey()` | `getSHAKey()` | `getSHAKey()` | `v1-required` | |
| Expanded DSP code | `getDSPCode()` | `getDSPCode()` | `getDSPCode()` | `v1-required` | |
| JSON | `getJSON()` | `getJSON()` | `getJSON()` | `v1-required` | |
| Compile options | `getCompileOptions()` | `getCompileOptions()` | `getCompileOptions()` | `v1-required` | |
| Library list | `getLibraryList()` | `getLibraryList()` | `getLibraryList()` | `v1-required` | |
| Include pathnames | `getIncludePathnames()` | `getIncludePathnames()` | `getIncludePathnames()` | `v1-required` | |
| Warning messages | `getWarningMessages()` | `getWarningMessages()` | `getWarningMessages()` | `v1-required` | |
| Create instance | `createDSPInstance()` | `createDSPInstance()` | `createDSPInstance()` | `v1-required` | |
| Memory manager set/get | `setMemoryManager/getMemoryManager` | same | same | `adapted` | C++ wrapper adapts `dsp_memory_manager` to the aligned C table and returns the effective legacy binding; invalid/conflicting replacement preserves the prior manager. |
| Target getter | `getTarget()` | n/a | `getTarget()`? | `decision-pending` | Depends on Cranelift target exposure semantics |
| `classInit(sample_rate)` | present | n/a | `classInit(sample_rate)`? | `decision-pending` | LLVM-only in wrapper |

### 4.4 C++ global/free function parity (factory/cache/serialization)

| Family | LLVM C++ API | Interpreter C++ API | Cranelift C++ target | Status | Notes |
|---|---|---|---|---|---|
| Version | `getCLibFaustVersion` | `getCLibFaustVersion` | `getCLibFaustVersion` | `v1-required` | shared C symbol |
| Cache get by SHA | `getDSPFactoryFromSHAKey` | `getInterpreterDSPFactoryFromSHAKey` | `getCraneliftDSPFactoryFromSHAKey` | `v1-required` | backend-prefixed C++ style expected |
| Create from file/string/signals/boxes | `createDSPFactoryFrom*` | `createInterpreterDSPFactoryFrom*` | `createCraneliftDSPFactoryFrom*` | `v1-required` | adapted shape: keep `opt_level`, omit LLVM-specific `target` |
| Delete factory | `deleteDSPFactory` | `deleteInterpreterDSPFactory` | `deleteCraneliftDSPFactory` | `v1-required` | |
| Cache list/delete-all | `getAllDSPFactories`, `deleteAllDSPFactories` | `getAllInterpreterDSPFactories`, `deleteAllInterpreterDSPFactories` | `getAllCraneliftDSPFactories`, `deleteAllCraneliftDSPFactories` | `v1-required` | |
| MT start/stop | `startMTDSPFactories`, `stopMTDSPFactories` | same | same | `v1-required` | shared C symbol |
| Bitcode read/write | `readDSPFactoryFromBitcode*`, `writeDSPFactoryToBitcode*` | `readInterpreterDSPFactoryFromBitcode*`, `writeInterpreterDSPFactoryToBitcode*` | `readCraneliftDSPFactoryFromBitcode*`, `writeCraneliftDSPFactoryToBitcode*` | `v1-required` | |
| IR read/write | `readDSPFactoryFromIR*`, `writeDSPFactoryToIR*` | n/a | `readCraneliftDSPFactoryFromIR*`, `writeCraneliftDSPFactoryToIR*` | `v1-deferred` | Deferred in V1 **without exported symbols** |
| Machine/object read/write | `readDSPFactoryFromMachine*`, `writeDSPFactoryToMachine*`, `writeDSPFactoryToObjectcodeFile` | n/a | Cranelift equivalents TBD | `v1-deferred` | Deferred in V1 **without exported symbols** |
| Foreign function registration | `registerForeignFunction` | n/a | `registerCraneliftForeignFunction` | `v1-deferred` | Naming locked; family deferred in V1 |
### 4.5 Remaining Phase 0 work for C++ wrappers

- Confirm exact naming of Cranelift C++ free functions for LLVM-only families
  (`getDSPMachineTarget`, IR/machine/object helpers, foreign-function registration).
- Freeze adapter behavior where Cranelift cannot implement an LLVM-specific
  backend artifact in V1 (real implementation vs typed unsupported).
- Add a final "exact signature" appendix for the chosen Cranelift C++ wrappers
  after the remaining LLVM-only helper naming decisions are resolved.

---

## 5. Open Decisions (Must be Resolved Before Deep Implementation)

1. **Machine target getter semantics (post-V1 if reintroduced)**
   - meaning/format for Cranelift target exposure if target getters are added after V1

Per repository collaboration rule, these require explicit requester confirmation
before implementation proceeds on the affected function families.
