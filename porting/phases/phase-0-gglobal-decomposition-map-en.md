# Phase 0 — `gGlobal` Decomposition Map (Critical Flow)

> Scope: first decomposition pass for the effective production flow, aligned with Phase 0 Gate work.
> Source of truth (C++): `/Users/letz/Developpements/RUST/faust`

## 1. Scope and analyzed files

This map targets the currently active compile path and parser-critical flow:

- `/Users/letz/Developpements/RUST/faust/compiler/global.hh`
- `/Users/letz/Developpements/RUST/faust/compiler/global.cpp`
- `/Users/letz/Developpements/RUST/faust/compiler/parser/faustparser.y`
- `/Users/letz/Developpements/RUST/faust/compiler/parser/faustlexer.l`
- `/Users/letz/Developpements/RUST/faust/compiler/libcode.cpp`
- `/Users/letz/Developpements/RUST/faust/compiler/generator/instructions_compiler.cpp`
- `/Users/letz/Developpements/RUST/faust/compiler/generator/instructions_compiler1.cpp`
- `/Users/letz/Developpements/RUST/faust/compiler/generator/instructions_compiler_jax.cpp`
- `/Users/letz/Developpements/RUST/faust/compiler/generator/dag_instructions_compiler.cpp`

`faustlexer.l` has no direct `gGlobal->` access, but is part of the parser flow boundary.

Reference touch counts (`gGlobal->*` occurrences) in analyzed files:

| File | Occurrences |
|---|---:|
| `parser/faustparser.y` | 65 |
| `parser/faustlexer.l` | 0 |
| `libcode.cpp` | 299 |
| `generator/instructions_compiler.cpp` | 184 |
| `generator/instructions_compiler1.cpp` | 3 |
| `generator/instructions_compiler_jax.cpp` | 7 |
| `generator/dag_instructions_compiler.cpp` | 23 |

Observed write hotspots (first-pass):

- Parser writes: `gResult`, `gWaveForm`, `gLstDependenciesSwitch`, `gLstDistributedSwitch`, `gStripDocSwitch`, `gErrorCount`.
- Orchestration writes (`libcode.cpp`): backend capability toggles (`gAllowForeign*`, `gFAUSTFLOAT2Internal`, `gNeedManualPow`, `gRemoveVarAddress`, `gUseDefaultSound`, `gLoopVarInBytes`, `gWaveformInDSP`, `gMachinePtrSize`, `gBool2Int`, `gExtControl`), API flow state (`gInputString`, `gInputFiles`, `gDSPFactory`, `gErrorMessage`).
- Codegen writes: `gSTEP` increment and `gTablesSize` updates in `instructions_compiler.cpp`.

## 2. Deliverable A — Field-to-context mapping table

This table maps `gGlobal` responsibilities used in the critical flow to explicit Rust contexts and owning crates.

| Target context (Rust) | Owning crate | `gGlobal` fields/methods in critical flow | Notes |
|---|---|---|---|
| `CompilerConfig` (immutable request options) | `compiler` (+ shared config type in `utils` if needed) | `gOutputLang`, `gClassName`, `gSuperClassName`, `gProcessName`, `gFloatSize`, `gVectorSwitch`, `gSchedulerSwitch`, `gOpenCLSwitch`, `gCUDASwitch`, `gMemoryManager`, `gExtControl`, `gOneSample`, `gOneSampleIO`, `gInPlace`, `gComputeMix`, `gBool2Int`, `gLocalCausalityCheck`, `gDumpNorm`, `gStrictSelect`, `gSimplifySelect2`, `gUseDenseDelay`, `gMaxDenseDelay`, `gMinDensity`, `gMaxCopyDelay`, `gMinCopyLoop`, `gMaskDelayLineThreshold`, `gIIRRingThreshold`, `gFirLoopSize`, `gHLSUnrollFactor`, `gFactorizeFIRIIRs`, `gReconstructFIRIIRs`, `gSchedulingStrategy`, `gPrintXMLSwitch`, `gPrintDocSwitch`, `gPrintFileListSwitch`, `gDrawPSSwitch`, `gDrawSVGSwitch`, `gDrawSignals`, `gDrawRetiming`, `gDrawRecProjGraph`, `gGraphSwitch`, `gTopoSwitch`, `gPrintHSchedule`, `gExportDSP`, `gTimeout` | Replace mutable option toggles by one immutable config snapshot per compile request. |
| `PathConfig` + `InputSet` | `compiler` | `gInputString`, `gInputFiles`, `gOutputFile`, `gOutputDir`, `gArchFile`, `gInjectFlag`, `gInjectFile`, `gMasterDocument`, `gMasterName`, `gReader` | Keep filesystem and input orchestration in `compiler`; parser receives concrete source units. |
| `ParserCtx` (parse-local mutable state) | `parser` | `gResult`, `gWaveForm`, `gStripDocSwitch`, `gLstDependenciesSwitch`, `gLstDistributedSwitch`, parser-side error increments, `nil` usage in grammar actions | Parser-specific mutable state must stop leaking into codegen/orchestration state. |
| `DiagnosticsCtx` | `diagnostics` (data), coordinated by `compiler` | `gErrorCount`, `gErrorMessage`, global warning state | Normalize to structured diagnostics and status instead of string side channels. |
| `TreeArenaCtx` (interning + list/tree primitives + properties) | `tlib` | `nil`, `cons` ecosystem, tree/property identities (`BOXTYPEPROP`, `DEFLINEPROP`, `USELINEPROP`, etc.) as parser/eval/codegen dependencies | Core identity model should be session-owned and passed explicitly. |
| `PrimitiveRegistry` (math/box primitive handles) | `boxes` + `signals` on top of `tlib` | `gAbsPrim`, `gAcosPrim`, `gAsinPrim`, `gAtanPrim`, `gAtan2Prim`, `gCosPrim`, `gSinPrim`, `gTanPrim`, `gExpPrim`, `gLogPrim`, `gLog10Prim`, `gPowPrim`, `gSqrtPrim`, `gMinPrim`, `gMaxPrim`, `gFmodPrim`, `gRemainderPrim`, `gFloorPrim`, `gCeilPrim`, `gRintPrim`, `gRoundPrim` | Parser grammar currently dereferences prim pointers directly; move to explicit registry passed to parser/boxes constructors. |
| `MetadataCtx` | `compiler` (+ `doc` integration) | `gMetaDataSet`, `gFunMDSet` | Metadata is currently touched in parser and consumed later in libcode/codegen; isolate as explicit artifact in compile session. |
| `CodegenCtx` (mutable lowering state) | `codegen` | `getFreshID`/`gIDCounters`, `gSTEP`, `gTablesSize`, `initTypeSizeMap`/`gTypeSizeMap`, `gMachinePtrSize` (read path), backend-sensitive sizing | Must be explicit per backend compilation path and not shared globally across requests. |
| `BackendProfile` (capability toggles, derived from config + backend kind) | `codegen` (selected by `compiler`) | `gAllowForeignFunction`, `gAllowForeignConstant`, `gAllowForeignVar`, `gFAUSTFLOAT2Internal`, `gNeedManualPow`, `gRemoveVarAddress`, `gUseDefaultSound`, `gHasTeeLocal`, `gLoopVarInBytes`, `gWaveformInDSP` | `libcode.cpp` currently mutates these inline per backend branch; should become declarative backend descriptors. |
| `ApiSessionState` (public API call lifecycle) | `compiler` (+ future `cffi`) | `gDSPFactory`, `reset()`, `initDirectories()`, `processCmdline()`, `initDocumentNames()`, `parseSourceFiles()` sequencing | API endpoints currently coordinate through mutable singleton state; replace by per-request session objects. |

## 3. Deliverable B — Unresolved coupling list

These couplings remain risky after first-pass mapping and must be resolved before deep parser/codegen migration:

1. `gOutputLang` is used both for top-level backend dispatch and deep codegen behavior branches.
2. `gFloatSize` crosses parser filtering (`variant` acceptance) and backend type/lowering behavior.
3. `gMetaDataSet` is written during parsing and read in later orchestration/codegen stages without explicit artifact boundaries.
4. `gErrorCount`/`gErrorMessage` mix parser errors, orchestration failures, and API return paths.
5. `nil` and list construction semantics are shared parser/eval/codegen assumptions with implicit singleton access.
6. `getFreshID` and `gSTEP` are global mutable counters used across compilers; determinism and isolation are not explicit.
7. Backend branches in `libcode.cpp` mutate capability flags (`gAllowForeign*`, `gNeedManualPow`, etc.) as side effects.
8. `gMachinePtrSize` is mutated for wasm code paths and reused by type-size initialization.
9. Parser doc/listing switches (`gStripDocSwitch`, `gLst*`) are parser-local concerns stored in global scope.
10. Lifecycle operations (`reset/init/processCmdline/parseSourceFiles`) are not encapsulated in one explicit request/session contract.
11. Visitor singletons under backend compile flags (`gWASMVisitor`, `gInterpreterVisitor`, etc.) imply hidden shared state across sub-containers.
12. Path/source reading (`gReader`) is shared through global state and reused in reporting (`listSrcFiles`) after parsing.

### 3.1 Blocking classification for next gates

Legend:
- `MUST`: must be resolved before entering target gate.
- `DEFER`: can be deferred with explicit mitigation and owner.

| # | Coupling (short) | Target gate | Blocking status | Mitigation / next action |
|---:|---|---|---|---|
| 1 | `gOutputLang` used for dispatch + deep codegen behavior | A.5 / B | DEFER | Keep in `CompilerConfig`; allow temporary read-through in codegen while introducing backend profiles in parallel. |
| 2 | `gFloatSize` crosses parser filter and lowering behavior | B | MUST | Freeze one explicit location for variant filtering (`ParserCtx` + config snapshot) and one for lowering (`CompilerConfig`). |
| 3 | `gMetaDataSet` write/read across phases | B | MUST | Define metadata artifact in `CompileSession`; parser writes only there, later phases consume immutable view. |
| 4 | `gErrorCount`/`gErrorMessage` mixed semantics | B | MUST | Introduce structured diagnostics sink and prohibit parser/codegen direct string-side effects. |
| 5 | `nil` and list semantics implicit singleton | A | MUST | Establish `tlib` ownership for `nil/cons` semantics and parser tests on list ordering before parser migration. |
| 6 | `getFreshID`/`gSTEP` global mutable counters | A.5 | MUST | Move counters to `CodegenCtx`; add deterministic ID generation tests. |
| 7 | `libcode.cpp` mutates backend capability flags inline | B | MUST | Replace with declarative backend profiles in orchestration layer; no per-branch mutable flag mutation in parser path. |
| 8 | `gMachinePtrSize` mutated for wasm and reused globally | B | DEFER | Keep behind backend target profile; isolate wasm override in backend-specific configuration. |
| 9 | Parser doc/listing switches in global scope | B | MUST | Move to `ParserCtx`; ensure no codegen dependency on parser local switches. |
| 10 | Lifecycle operations not one explicit request/session contract | A.5 / B | MUST | Formalize `CompileSession` lifecycle API before parser integration into production crate. |
| 11 | Backend visitor singletons imply hidden shared state | post-B | DEFER | Track as backend integration debt; not required for parser viability gate. |
| 12 | Shared `gReader` state reused across reporting and parse | B | MUST | Scope reader state per request (`PathConfig` + reader instance) and pass outputs as explicit artifacts. |

### 3.2 Gate readiness rule

- Enter Gate A only if all Gate A `MUST` items are resolved.
- Enter Gate A.5 only if Gate A.5 `MUST` items are resolved.
- Enter Gate B only if all Gate B `MUST` items are resolved or downgraded with explicit sign-off in Phase 0 Go/No-Go.

## 4. Deliverable C — Crate-boundary contract draft

This is the first explicit contract for the migrated production flow.

### 4.1 `compiler` crate (orchestration owner)

- Owns `CompileRequest`, `CompilerConfig`, `PathConfig`, `CompileSession`.
- Builds immutable config and explicit per-request mutable session stores.
- Performs backend selection through declarative backend descriptors (not mutable flag mutation in dispatch branches).

### 4.2 `tlib` crate (identity and properties owner)

- Owns `TreeArenaCtx` and list/tree/property primitives.
- Exposes explicit APIs for interning, list construction, and property access.
- No compile-option state, no filesystem state.

### 4.3 `boxes` crate (box construction owner)

- Owns box constructors and parser-targeted builder entry points.
- Depends on `tlib` and `PrimitiveRegistry`.
- Must be usable directly by parser semantic actions with no parser-local placeholder layer.

### 4.4 `parser` crate (frontend owner)

- Owns lexer/parser and `ParserCtx`.
- Produces parse artifacts (`expanded_defs`, diagnostics, parser metadata deltas) without touching codegen/backend state.
- Consumes `boxes` + `tlib` explicit contexts only.

### 4.5 `codegen` crate (lowering/backends owner)

- Owns `CodegenCtx` and backend modules.
- Consumes immutable `CompilerConfig` + explicit IR/artifacts from previous phases.
- No direct dependence on parser-local mutable state.

### 4.6 `diagnostics` crate (diagnostics model owner)

- Owns diagnostic data model and categorization.
- `compiler` aggregates diagnostics from parser/eval/codegen and maps to API outputs.
- Owns stable error-code families and rendering contracts (human + machine-readable).
- Must replace `gErrorCount` / `gErrorMessage` side channels with structured diagnostics bundles.
- Reference architecture: `porting/faust-rust-diagnostics-model-en.md`.

## 5. Gate 0 pass statement (current status)

Status: **in progress**.

- Mapping table: drafted for critical flow fields and methods.
- Unresolved couplings: identified and prioritized.
- Crate-boundary contract: drafted.

Remaining before Gate 0 closure:

- Add per-field ownership for the remaining `global.hh` fields not exercised in the critical path.
- Validate the draft mapping against one concrete implementation spike (`TreeArena/tlib-core` + `boxes` subset).
- Link this map into Phase 0 Go/No-Go review checklist with owner/date per unresolved coupling.
