# Cmajor backend port and test plan — 2026-08-04

Status: scalar implementation in qualification. C1-C5 are implemented; C6 has
a narrow pinned-C++ differential, a stateful Cmajor runtime probe, and a
125-case impulse gate. Output-event/bargraph runtime remains open. C7-C8 are
deferred.

## 1. Goal and recommended scope

Port the Faust C++ Cmajor source backend to `faust-rs`, starting with the
production scalar path exposed as:

```sh
faust-rs -lang cmajor -cn mydsp input.dsp -o output.cmajor
```

The first completion target is deliberately narrower than the complete Cmajor
tooling shipped in the C++ repository:

- scalar `-lang cmajor` source generation from canonical FIR;
- `float32` and `float64` output;
- the intrinsic one-sample processing model and separated control path;
- Faust controls as Cmajor input events;
- bargraphs as rate-limited Cmajor output events;
- state, delays, loops, tables, waveform data, and generated subcontainers;
- architecture wrapping with `architecture/cmajor/minimal.cmajor`;
- stable diagnostics for every rejected construct;
- syntax, structural, differential, lifecycle, and numeric validation.

The following are follow-on milestones, not part of the first backend gate:

- `cmajor-poly` and the `poly-dsp*.cmajor` architecture wrappers;
- `cmajor-dsp`, used by the C++ `dsp` compatibility wrapper;
- `cmajor-hybrid` and parsing embedded `faust Name { ... }` blocks;
- the `faust2cmajor` application workflow, JUCE/CLAP project generation, player,
  tester, editor, and Cmajor SDK runtime wrappers;
- vector, scheduler, OpenMP, foreign functions, and soundfiles, which the C++
  backend itself rejects or does not implement.

Calling the first milestone “Cmajor backend complete” means complete for the
scalar `-lang cmajor` contract above. Full C++ Cmajor ecosystem parity must not
be claimed until the follow-on variants are implemented and gated separately.

## 2. Reference baseline and source inventory

### 2.1 Pins used for this analysis

The authoritative C++ Faust checkout for this plan is referred to below as
`<faust-cpp>`:

- branch: `master-dev-ocpp-od-fir-2-FIR19`;
- commit: `8eebea4294a44a5260484c750d332781ed9f8ffd`;
- Faust binary: `2.84.3`;
- Cmajor tool used to validate generated source: `1.0.3175`, build
  `2026-07-10`.

This is the same reference pin used by the general `faust-rs` port. A comparison
with the newer `master-dev` commit
`0513a67548e4c00cbad793c6265f245132d5fcca` confirms that the backend-specific
sources are effectively the same: the only local Cmajor-container change is the
removal of a `sortIO` call in the newer source. Newer generated programs also
reflect changes in the shared C++ lowering and scheduling pipeline, but they are
not a second parity target for this plan. Endpoint order and sample-order
behavior are taken from `8eebea429...` unless a later, explicit baseline update
is approved.

### 2.2 Core backend sources

| C++ source | Size | Contract owned |
|---|---:|---|
| `compiler/generator/cmajor/cmajor_code_container.cpp` | 504 lines | processor assembly, lifecycle, subcontainers, one-sample loop, bargraph cadence |
| `compiler/generator/cmajor/cmajor_code_container.hh` | 138 lines | scalar container, rejected modes, table-size specialization visitors |
| `compiler/generator/cmajor/cmajor_instructions.hh` | 657 lines | types, statements, expressions, math mapping, streams, UI events, metadata |
| `compiler/generator/type_manager.hh` (`CmajorStringTypeManager`) | shared | Cmajor scalar and array type spelling |
| `compiler/libcode.cpp` (`compileCmajor`) | shared | backend dispatch and forced compiler modes |
| `compiler/global.cpp` | shared | option compatibility and internal math-function policy |

Important reference points at `8eebea429...` are:

- `libcode.cpp:720-748`: effective Cmajor compiler configuration;
- `cmajor_code_container.cpp:54-76`: processor-name and scalar-mode checks;
- `cmajor_code_container.cpp:157-196`: lifecycle emitted by C++;
- `cmajor_code_container.cpp:198-422`: processor section assembly;
- `cmajor_code_container.cpp:424-466`: one-sample `main()` loop;
- `cmajor_instructions.hh:34-221`: UI endpoint planner;
- `cmajor_instructions.hh:223-641`: FIR instruction emission;
- `type_manager.hh:338-386`: direct type mapping.

### 2.3 Architectures, applications, and tests

| Source | Role |
|---|---|
| `architecture/cmajor/minimal.cmajor` | minimal `<<includeclass>>` wrapper |
| `architecture/cmajor/poly-dsp.cmajor` | polyphonic voice graph and MIDI/MPE routing |
| `architecture/cmajor/poly-dsp-effect.cmajor` | polyphonic graph plus effect processor |
| `tools/faust2appls/faust2cmajor` | patch manifest, poly/effect composition, play/render/JUCE workflow |
| `tests/impulse-tests/Make.cmajor` | Cmajor-to-C++ impulse test pipeline |
| `tests/impulse-tests/archs/impulsecmajor.cpp` | thin host for generated Cmajor C++ |
| `tests/impulse-tests/cmajordsp.cmajorpatch` | patch manifest consumed by `cmaj generate` |
| `architecture/cmajor/cmajor-tools.h` | pure Faust and hybrid Cmajor/Faust file preparation |
| `architecture/faust/dsp/cmajorpatch-dsp.h` | runtime-backed Faust `dsp` wrapper |
| `architecture/faust/dsp/cmajor-cpp-dsp.h` | generated-C++ Faust `dsp` wrapper |

These files are relevant to parity analysis, but they must not all be pulled
into the core emitter. In particular, the code generator must not depend on the
Cmajor SDK or invoke `cmaj` itself.

## 3. Effective C++ pipeline

The production C++ path is:

```text
parse -> boxes -> eval -> propagate -> normalize/type
      -> InstructionsCompiler -> CmajorCodeContainer -> Cmajor source
```

`compileCmajor` forces the following behavior before lowering:

| C++ setting | Meaning for the Rust port |
|---|---|
| foreign functions/constants/variables disabled | reject unsupported foreign nodes with typed diagnostics |
| `gBool2Int = true` | comparison results used numerically are explicitly converted |
| `gExtControl = true` | control-rate calculations live in `control()` |
| `gFAUSTFLOAT2Internal = true` | stream and UI types are the selected internal real type |
| `gOneSampleIO = true` | processor I/O is one stream value per tick |
| `gNeedManualPow = false` | emit Cmajor `pow`, including integer-exponent cases |

Although `compileCmajor` contains a vector compiler branch, container creation
rejects `-vec` before it can be used; the vector method is a TODO. OpenMP and
scheduler modes are also rejected. The parity target is therefore scalar, not
an unfinished vector source shape.

The `startWith("cmajor")` dispatch exposes four internal language spellings:

| Spelling | Difference from standard Cmajor |
|---|---|
| `cmajor` | default zone-based event names |
| `cmajor-poly` | shortname-based events used by the poly wrapper |
| `cmajor-hybrid` | shortname or `[cmajor:alias]` event names |
| `cmajor-dsp` | extra lifecycle event endpoints plus embedded UI JSON |

Only `cmajor` belongs to the first gate. Each other spelling changes a public
endpoint contract and requires its own tests; it is not a harmless alias.

## 4. Reference output contract

### 4.1 Processor shell and I/O

The normal output has this shape:

```cmajor
namespace faust
{
    processor mydsp
    {
        input stream float32 input0;
        output stream float32 output0;

        void main()
        {
            loop
            {
                if (fUpdated) { fUpdated = false; control(); }
                // one-sample FIR body
                output0 <- ...;
                advance();
            }
        }
    }
}
```

The processor name must not begin with a digit. Inputs and outputs are Cmajor
streams, stores to output streams use `<-`, and each sample ends with exactly
one `advance()`. `main()` is the target runtime entry point; there is no block
`compute(count, inputs, outputs)` API.

The C++ backend emits the misspelled helper `getNumOuputs()`. It is part of the
observable generated source and must be preserved initially. A correctly
spelled alias may only be added after a compatibility decision confirms that
the extra symbol is wanted; silently “fixing” the name would not be 1:1 parity.

### 4.2 Types, expressions, and statements

The primary type mappings are:

| FIR type | Cmajor type |
|---|---|
| `i32` | `int32` |
| `i64` | `int64` |
| `f32` | `float32` |
| `f64` | `float64` |
| boolean | `bool` |
| fixed-size array | `T[N]` |
| void | `void` |

The C++ type manager contains nominal quad and fixed-point spellings, but the
global option model does not support those modes for Cmajor. The first Rust
port supports only `float32` and `float64`; fixed point and quad must fail before
emission with a stable capability diagnostic.

Parity-sensitive syntax includes:

- casts as `type (expression)`;
- conditions as `bool (expression)`;
- constant array indexing as `[constant]`;
- dynamic array indexing as `.at (index)`, preserving Cmajor wrap behavior;
- indexed struct addresses as named fields;
- ordinary scalar `if` and `for` constructs, including reverse loops;
- the Cmajor math-name table and target built-ins;
- local helpers for `copysign` and `round` in the selected precision;
- empty `DeclareFun` emission in the old visitor only where the required
  implementation is assembled elsewhere by the container;
- explicit rejection of bitcasts and every unknown FIR node.

The Rust emitter must never reproduce the C++ `faustassert(false)` behavior for
bitcasts. It must return a typed backend error instead.

### 4.3 UI endpoints and separated control

Buttons, checkboxes, sliders, and numeric entries become `input event`
endpoints. Each handler updates the zone and sets `fUpdated` only if the value
changed. The next audio tick clears `fUpdated` and calls `control()` once for
all accumulated changes.

Endpoint annotations carry:

- a normalized short name;
- the full UI group path;
- slider `min`, `max`, `init`, and `step`;
- button/checkbox `boolean`, `text`, and optional `latching` attributes;
- Faust metadata as `meta_<fresh-key>` annotations.

`buildLabel` replaces spaces, parentheses, backslashes, slashes, dots, and
hyphens with underscores. Metadata keys beginning with a digit are not emitted.
Repeated metadata keys receive deterministic fresh suffixes. UI traversal is a
two-pass operation because short names depend on the full set of widget paths.

For standard `cmajor`, the public endpoint name is zone-based, for example
`eventfHslider0`; the short name is still used in the annotations. That naming
is consumed by `cmajorpatch-dsp.h` and `cmajor-cpp-dsp.h`, so endpoint spelling
and ordering are contract tests, not formatting snapshots.

### 4.4 Bargraphs

Bargraphs become `output event` endpoints, while their zones remain processor
fields. A store performs both operations:

1. update the zone every sample;
2. when `fControlSlice == 0`, send the zone through its output event.

`fControlSlice` is initialized and reset from
`int(processor.frequency) / 50`, giving an approximately 50 Hz meter update
rate. The sample loop decrements the counter once per sample. Tests must cover
both the stored value and event cadence; checking only the endpoint declaration
would miss the functional contract.

### 4.5 Tables and subcontainers

Cmajor fixed-size array types require the concrete table length in helper
signatures. The C++ backend therefore:

- discovers each `fill...` call and its constant table size;
- clones the call name with a size suffix, for example
  `filltableSIG0_65536`;
- emits a matching `float32[65536]& table` parameter;
- emits generated table DSPs as `struct` values plus functions taking
  `Struct& this`;
- stores tables as processor instance fields because the current Cmajor model
  does not provide the C++ backend's shared static storage model.

The Rust port must first inspect the canonical FIR shape. If the Rust FIR
already carries concrete table parameter types, it should emit them directly.
If one logical fill function is called with multiple lengths, add a deterministic
`CmajorTablePlan` collection/specialization pass over FIR. Do not introduce an
index side table detached from the owning function/call unless the need and its
invariants are documented and structurally tested.

Implementation finding (2026-08-04): canonical Rust FIR does carry concrete
array element types and lengths at every Cmajor emission boundary exercised by
read-only, writable, waveform, and generated-table fixtures. The backend emits
these owned types directly. Two generator sizes compiled in one process and a
repeated first request produce deterministic source accepted by Cmajor
1.0.3175, so `CmajorTablePlan` is not introduced. This is an `adapted`
representation-level mapping: it removes C++ placeholder-type repair without
changing the generated fixed-size table contract.

### 4.6 Lifecycle: intentional adaptation

The current C++ Cmajor output is:

```text
init -> instanceInit
instanceInit -> classInit -> instanceConstants
             -> instanceResetUserInterface -> instanceClear
```

The comments explain that `classInit` is called per processor instance because
tables are not shared. This conflicts with the repository-wide
`porting/backend-lifecycle-contract-en.md`, which requires:

```text
init -> classInit -> instanceInit
instanceInit -> instanceConstants -> instanceResetUserInterface -> instanceClear
```

The Rust backend mapping is therefore `adapted`, not `1:1`:

- retain tables as fields of each Cmajor processor instance;
- make `init()` call `classInit(sample_rate)` once for that processor instance,
  then call `instanceInit(sample_rate)`;
- make direct `instanceInit()` omit `classInit`;
- use the compiled lifecycle FIR bodies as the only initialization/clearing
  authority;
- do not add an emitter-side heuristic that clears fields by name.

This preserves the actual per-instance table behavior while satisfying the
shared lifecycle ordering. A backend-specific lifecycle conformance test is a
hard prerequisite for adding Cmajor to impulse, golden, or parity gates.

### 4.7 Unsupported reference behavior

The first port must reject, with stable `FRS-CGEN-CMAJ-*` or execution-option
diagnostics:

- vector, scheduler, and OpenMP requests;
- soundfile UI nodes;
- arbitrary foreign functions, constants, and variables;
- bitcasts and unsupported pointer forms;
- fixed-point and quad precision;
- invalid processor identifiers;
- malformed or unknown FIR roots/nodes.

The generator must not emit a partial Cmajor file after encountering one of
these cases.

## 5. Reference probes completed during planning

The following programs were generated with Faust C++ `2.84.3` at the pinned
commit and then accepted
by `cmaj generate --target=syntaxtree` from Cmajor `1.0.3175`:

| Probe | Contract exercised | Result |
|---|---|---|
| `noise.dsp` | state, integer recurrence, slider, separated control | accepted |
| `UITester.dsp` | all UI widgets, paths, metadata, many streams, bargraphs | accepted |
| table oscillator | subcontainer, `fill..._<size>`, dynamic `.at`, 65,536-value table | accepted |
| double oscillator | `float64` streams, events, tables, and math | accepted |
| poly oscillator/noise | deferred `cmajor-poly` path | pinned branch asserts in `backendDelayType`; excluded from the scalar gate and recorded for C7 |

These probes are evidence that the analyzed C++ source is accepted by the
current local Cmajor tool for the scalar target, not versioned test artifacts.
The `cmajor-poly` assertion is evidence that the custom pinned branch is not a
usable poly runtime oracle without separate repair or rebaselining; it does not
block C0-C6. Phase C0 must turn the scalar probes into compact,
repository-local Faust fixtures and reproducible scripts. Tests must not depend
on an installed Faust library tree; library-like definitions must be test-local
and compiled through `compile_source_to_*` APIs.

## 6. Current `faust-rs` baseline and reuse map

### 6.1 Existing pieces

`faust-rs` currently has only a Cmajor scaffold:

- `crates/codegen/src/backends/cmajor/mod.rs` exposes `BACKEND_NAME` and
  `backend_id()`;
- `crates/codegen/README.md` correctly lists Cmajor as scaffolded;
- no `CliLang::Cmajor`, compiler facade entry point, lowering dispatcher, typed
  compiler error, fixture mode, or source-mode branch exists;
- the execution capability table explicitly waits to add scaffolded backends
  until they become active.

The important reusable infrastructure already exists:

- canonical signal-to-FIR lowering;
- `ControlRateMode::External` and `ProcessingApi::OneSample`;
- execution-capability validation;
- FIR lifecycle functions and module sections;
- FIR UI nodes and shared UI short-name computation;
- scalar state, arrays, loops, reverse loops, table, and waveform FIR nodes;
- typed backend errors and diagnostic mapping patterns;
- compiler facade/source/file helpers;
- generic architecture marker processing, currently surfaced as
  `wrap_cpp_with_architecture` but already tested for Julia templates;
- Codebox tests that demonstrate how to force intrinsic one-sample/external
  control modes and how to compare a source emitter numerically with the
  interpreter.

### 6.2 Recommended lowering contract

Cmajor should follow the Codebox lowering pattern:

```text
signals
  -> force ControlRateMode::External
  -> force ProcessingApi::OneSample
  -> validate scalar ComputeMode
  -> transform fast-lane FIR
  -> FIR verifier
  -> generate_cmajor_module
```

Add a `cmajor` execution-capability row with:

- external control: `Intrinsic`;
- one-sample: `Intrinsic`;
- combined: `Intrinsic`;
- vector: `Unsupported`;
- canonical block compute required: `false`.

Passing `-ec`, `-os`, both, or neither must produce byte-identical Cmajor source.
Passing `-vec` must fail by backend name rather than silently emitting scalar
code.

### 6.3 Proposed Rust API

The public codegen API is `adapted`:

```rust
pub struct CmajorOptions {
    pub class_name: String,
    pub real_type: CmajorRealType,
}

pub enum CmajorRealType {
    Float32,
    Float64,
}

pub fn generate_cmajor_module(
    store: &FirStore,
    module: FirId,
    options: &CmajorOptions,
) -> Result<String, CodegenError>;
```

Start with a focused emitter in `crates/codegen/src/backends/cmajor/`. Split UI,
table planning, or syntax helpers into submodules when their independent
invariants justify it. Do not create a speculative generic “all text backends”
abstraction. Reuse stable shared helpers such as short-name computation; share
new Cmajor/Codebox syntax machinery only after both implementations demonstrate
the same semantics.

Recommended backend error classes are:

| Stable code | Meaning |
|---|---|
| `FRS-CGEN-CMAJ-0001` | FIR root is not a module |
| `FRS-CGEN-CMAJ-0002` | unsupported FIR construct or type |
| `FRS-CGEN-CMAJ-0003` | invalid Cmajor processor or endpoint identifier |
| `FRS-CGEN-CMAJ-0004` | inconsistent module/lifecycle/I/O structure |
| `FRS-CGEN-CMAJ-0005` | table specialization cannot be resolved |

The exact enum names may be Rust-idiomatic, but these machine-readable strings
must remain stable once tests and external callers consume them.

## 7. API and behavior mapping

| C++ surface | Rust target | Status | Compatibility rule |
|---|---|---|---|
| `compileCmajor` dispatch | compiler Cmajor lowering dispatcher | adapted | same forced modes through typed context, no globals |
| `CmajorCodeContainer::createContainer` | options/capability validation | adapted | same scalar and identifier restrictions |
| `CmajorScalarCodeContainer::generateCompute` | FIR frame-to-`main` emitter | adapted | same one-sample observable behavior |
| `CmajorInstVisitor` | Cmajor FIR text emitter | adapted | syntax/semantics preserved, typed errors replace asserts |
| `CmajorInstUIVisitor` | deterministic UI endpoint plan | adapted | names, annotations, paths, metadata, order preserved |
| `CmajorStringTypeManager` | Cmajor type/literal functions | adapted | same supported scalar/array output |
| table-size visitors | concrete owned FIR array types (no side table) | adapted | identical concrete helper signatures and table values |
| `-lang cmajor` CLI | `CliLang::Cmajor` | 1:1 | same user-facing spelling and text artifact |
| `-cn` processor name | `CmajorOptions::class_name` | 1:1 | same externally visible processor name |
| `-single` / `-double` | `CmajorRealType` | 1:1 | exact stream, UI, state, helper precision |
| `-a minimal.cmajor` | generalized architecture wrapping | 1:1 | same marker insertion contract |
| lifecycle call graph | generated lifecycle methods | adapted | per-instance tables retained, shared lifecycle ordering enforced |
| `cmajor-poly` | later flavor and architecture milestone | deferred | no false CLI alias before endpoint tests exist |
| `cmajor-dsp` | later compatibility flavor | deferred | requires lifecycle-event and JSON contract tests |
| `cmajor-hybrid` | hybrid parser/tooling milestone | deferred | requires embedded-source ownership and diagnostic design |
| `faust2cmajor`/SDK tools | separate application/runtime work | deferred | core emitter remains SDK-independent |
| vector/scheduler/OpenMP | capability rejection | 1:1 | C++ rejects these modes |
| soundfile | typed unsupported error | 1:1 | C++ rejects it |

Every implemented public surface must repeat its source provenance and mapping
status in Rustdoc. Deferred items must remain visibly deferred in the backend
README and the relevant porting journal entry.

## 8. Implementation phases

### C0 — Validation and scope freeze

Deliverables:

- pin the C++ reference and the supported Cmajor tool version range;
- add compact local DSP fixtures for identity, recurrence, delay, UI, bargraph,
  table, math, and double precision;
- capture normalized C++ source contracts from `8eebea429...`;
- capture any informative `master-dev` comparison separately, without making it
  a second pass/fail oracle;
- inspect actual Rust FIR for every fixture, especially lifecycle sections,
  one-sample I/O, tables, subcontainers, and UI ordering;
- record a Cmajor-specific `gGlobal` decomposition map using typed Rust fields;
- freeze accepted and rejected CLI options;
- decide whether `getNumOuputs()` remains the sole helper or is accompanied by
  a corrected alias; the parity default is to preserve only the existing name;
- confirm no new TreeArena pattern is needed. If the emitter adds only bounded
  FIR traversal, cite the existing Phase 0 benchmark instead of rerunning an
  unrelated arena benchmark.

Pass criteria:

- every first-gate feature has a C++ fixture and expected Rust FIR shape;
- every known difference is classified as formatting, intentional adaptation,
  or semantic blocker;
- lifecycle/table ownership is accepted explicitly;
- no prototype stub or unowned test gap remains;
- a written Go/No-Go decision is added to the implementation journal.

### C1 — Core emitter and processor shell

Deliverables:

- real `CmajorOptions`, real type, typed error, and
  `generate_cmajor_module` API;
- module decode and FIR verification assumptions;
- namespace, processor, field, lifecycle, `control`, and one-sample `main`
  sections in deterministic order;
- scalar streams, constants, loads/stores, casts, basic arithmetic, conditions,
  blocks, `if`, and loops;
- `float32` shell first, with unknown nodes rejected;
- source-provenance Rustdoc.

Pass criteria:

- identity, arithmetic, recurrence, and fixed-delay sources are accepted by
  `cmaj generate --target=syntaxtree`;
- one and only one `advance()` is emitted per tick;
- input/output arity and endpoint order match the pinned C++ reference;
- the backend-specific lifecycle structural test passes;
- all unsupported nodes fail before partial output is returned.

### C2 — Complete scalar FIR syntax and math

Deliverables:

- complete supported FIR statement/expression inventory;
- Cmajor math-name map and precision-correct literals;
- `copysign` and `round` helpers;
- constant versus dynamic array indexing;
- forward and reverse loops;
- state/delay/waveform coverage;
- explicit errors for bitcast, foreign, fixed/quad, and invalid pointer shapes.

Pass criteria:

- one focused structural and one numeric test covers each language-specific
  mapping whose wrong spelling could change behavior;
- delay and recurrence results match the interpreter;
- the current Cmajor frontend accepts the math and indexing corpus;
- a mutation removing `.at`, boolean conversion, or a math-name mapping causes
  at least one test to fail.

### C3 — UI, external control, metadata, and bargraphs

Deliverables:

- two-pass UI endpoint collection using shared short names;
- button, checkbox, slider, numeric-entry, and metadata annotations;
- deterministic zone-based endpoint handlers;
- `fUpdated` aggregation and one control call per changed tick;
- bargraph output events and 50 Hz control-slice logic;
- duplicate/invalid endpoint detection with typed diagnostics.

Pass criteria:

- endpoint names, types, order, paths, ranges, and metadata match C++ structural
  contracts;
- four facade shapes (neither flag, `-ec`, `-os`, both) emit identical text;
- event changes become audible on the next tick and do not rerun `control()`
  when unchanged;
- a rate-aware bargraph test verifies cadence at two sample rates;
- the UI tester is accepted by Cmajor `1.0.3175` or the newly pinned supported
  version.

### C4 — Tables, subcontainers, and double precision

Deliverables:

- table ownership and concrete-size helper signatures;
- deterministic fill-function specialization when required;
- subcontainer structs and `Struct& this` access;
- static/init and post-init FIR bodies in the correct lifecycle phase;
- full `float64` output;
- tests for multiple table lengths and repeated compilation in one process.

Pass criteria:

- table data is initialized once per processor `init`, not on direct
  `instanceInit`;
- two processor instances do not share mutable table state;
- 32-bit and 64-bit table/oscillator fixtures are accepted and run;
- two calls to the compiler facade produce deterministic, request-local output
  with no leaked fresh IDs or table sizes;
- direct `instanceInit` still satisfies the shared lifecycle test.

### C5 — Compiler facade, CLI, JSON, and architecture wrapping

Deliverables:

- Cmajor variants in compiler errors, diagnostic mapping, facade source/file
  helpers, emitters, and public re-exports;
- `CliLang::Cmajor`, canonical backend ID, source mode, fixture mode, and CLI
  transcript coverage;
- intrinsic execution-capability row and rejection tests;
- `-single`, `-double`, `-cn`, `-o`, `--json`, and signal-FIR lane plumbing;
- generalize `wrap_cpp_with_architecture` to a language-neutral text wrapper or
  add an equivalently tested Cmajor wrapper without duplicating marker parsing;
- minimal Cmajor architecture fixture using repository-relative paths;
- codegen README status update.

Pass criteria:

- facade and CLI emission are byte-identical for the same options;
- `faust-rs -lang cmajor ...` writes valid source to stdout or `-o`;
- `--json` produces the usual companion without changing Cmajor event output;
- `-a minimal.cmajor` inserts source at `<<includeclass>>` and handles CRLF and
  Windows paths;
- `-vec` and unsupported architecture/precision combinations fail with stable
  diagnostics;
- existing CLI/backend paths remain unchanged.

### C6 — Differential, runtime, golden, and cost gates

Deliverables:

- stable Cmajor source snapshots for the compact corpus;
- a narrow semantic normalizer for C++/Rust differential comparison;
- opt-in current-Cmajor frontend validation;
- Cmajor-to-C++ impulse runner based on the upstream `Make.cmajor` route;
- interpreter-versus-Cmajor numeric corpus;
- optimization parity at Cmajor `-O0` and `-O4` on a representative stateful
  subset;
- metadata recording the Faust C++ pin, Cmajor version, flags, and tolerances;
- compile-budget measurements after all codegen/compiler wiring lands.

Pass criteria:

- pure Rust tests pass on Linux, macOS, and Windows without external Cmajor;
- the external frontend gate accepts every positive fixture and rejects the
  negative Cmajor mutations;
- Cmajor `-O0` and `-O4` agree within the same numeric tolerance;
- impulse outputs match the canonical reference corpus;
- `cargo run -p xtask -- golden-check` and the C++ parity guardrails remain
  green;
- `cargo run --release -p xtask -- compile-budget-check` reports no unexplained
  regression;
- no golden baseline is raised to hide a failure.

Implementation status on 2026-08-04: the narrow observable contract matches
the pinned C++ backend for stream I/O, UI events, bargraphs, tables, and double
precision. The normalizer deliberately excludes formatting, the documented
adapted lifecycle, and table declarations: the C++ optimizer folds the compact
constant-table fixture while canonical Rust FIR retains an equivalent concrete
array and `.at` access. Cmajor-generated C++ executes the recursive fixture
identically at `-O0` and `-O4`. This is not yet the complete C6 numeric matrix;
UI event delivery, bargraph cadence, table runtime values, and the impulse
corpus remain unchecked below.

The `tests/impulse-tests` lane is now active as the opt-in `make cmajor` target.
Against a 133-case C++-oracle corpus, 125 supported scalar-double programs pass
through Cmajor 1.0.3175 and match the canonical traces. The eight exclusions
are auditable in `known.mk`: the shared `subcontainer1` gap; `bs` (`count` is
invalid in one-sample mode); `sound` (unsupported soundfile); and `bells`,
`modulations`, `osci`, `tester`, and `tester2` (expression/initializer
materialization limits). The passing set includes UI, bargraphs, state, tables,
waveforms, and all current upsampling/downsampling impulse fixtures.

### C7 — Polyphonic and effect application layer

This phase starts only after C0-C6 are green.

Deliverables:

- `cmajor-poly` endpoint flavor;
- tested `poly-dsp.cmajor` and `poly-dsp-effect.cmajor` wrapping;
- configurable voice count without textual shell substitution hazards;
- note-on, note-off, velocity/gain/gate, and stereo routing tests;
- effect auto/manual composition tests;
- a typed Cmajor patch-manifest generator if the application workflow is
  brought into `faust-rs`.

Pass criteria:

- poly endpoint names match wrapper expectations;
- voice allocation and note release are tested at runtime;
- mono/stereo and effect arity mismatches fail explicitly;
- generated manifests are valid JSON and use portable relative source paths.

### C8 — Optional compatibility and hybrid tooling

Treat the following as independent proposals with their own Phase 0 decisions:

- `cmajor-dsp` lifecycle event endpoints and embedded JSON;
- `cmajorpatch-dsp`/`cmajor-cpp-dsp` host wrappers;
- hybrid Cmajor files containing embedded Faust blocks;
- player, tester, editor, file watching, and SDK integration;
- JUCE and CLAP generation orchestration.

These features introduce external SDK ownership, host lifecycle, filesystem,
and diagnostic-location questions. They must not be smuggled into the core text
emitter as convenience helpers.

## 9. Test architecture

### 9.1 Layer A — Pure Rust, mandatory on every platform

No installed Faust or Cmajor is permitted. Tests compile self-contained Faust
strings and inspect or evaluate the resulting FIR/Cmajor text.

Required groups:

- module section order, type spelling, literals, and identifiers;
- stream input/output declarations and `<-` stores;
- scalar expression/statement coverage;
- state, recurrence, fixed and variable delays;
- dynamic `.at` indexing and reverse loops;
- lifecycle order and direct `instanceInit` behavior;
- UI endpoint names, paths, metadata, and deterministic ordering;
- external-control change aggregation;
- bargraph declaration and counter logic;
- table specialization and per-instance ownership;
- single/double precision;
- typed negative cases;
- facade/CLI/output-file/architecture-wrapper parity;
- repeated and parallel compilations to expose leaked global state.

### 9.2 Layer B — C++ differential, opt-in locally and pinned in CI

Use `FAUST_CPP_BIN` rather than a hardcoded checkout path. Generate Cmajor from
the same self-contained source with both compilers.

Compare exactly where the contract is expected to be 1:1:

- processor and endpoint names;
- endpoint order and arity;
- Cmajor types;
- UI annotations and metadata order;
- table lengths and specialized helper names;
- math-call mapping;
- one-sample I/O and `advance()` placement.

Compare structurally where Rust is intentionally adapted:

- section formatting and local variable names;
- FIR-dependent statement scheduling;
- lifecycle call sites, where Rust must follow the shared contract rather than
  reproduce the C++ Cmajor exception.

The normalizer may remove generated-version comments and whitespace. It must
not erase endpoint ordering, lifecycle calls, stream direction, operators,
array sizes, or statement order.

### 9.3 Layer C — Current Cmajor frontend

Use `CMAJ_BIN` when available. The minimal validity gate is:

```sh
"$CMAJ_BIN" generate --target=syntaxtree input.cmajor --output=tree.json
```

Alternatively, use a small `.cmajorpatch` and `play --dry-run` when manifest
resolution itself is under test. Store only compact diagnostics/snapshots, not
large generated syntax-tree JSON.

This layer checks target syntax and name resolution. It does not prove numeric
correctness.

### 9.4 Layer D — Numeric execution and impulse parity

Port the useful shape of the upstream impulse rule:

```text
Faust source -> Rust Cmajor source -> .cmajorpatch
             -> cmaj generate --target=cpp
             -> thin impulse host -> canonical trace
```

Use the existing Faust four-pass/control scenario where applicable. Compare
against the interpreter and the canonical impulse references:

- integers, counters, and endpoint/event counts: exact;
- ordinary `float32` arithmetic/delay cases: absolute or relative error at most
  `1e-6`;
- transcendental `float32` cases: at most `4` ULP where ULP comparison is
  available, otherwise calibrated per-case and no looser than `1e-6` relative;
- `float64`: absolute or relative error at most `1e-12`;
- NaN/Inf/sign-of-zero classes: exact classification.

#### 9.4.1 `tests/impulse-tests` integration contract

The pinned C++ suite already treats Cmajor as an executable backend in
`tests/impulse-tests/Make.cmajor`. The Rust lane shall preserve its semantic
route:

```text
faust-rs -lang cmajor -double -cn cmajordsp
  -> cmaj generate --target=cpp cmajordsp.cmajorpatch
  -> generated cmajordsp C++ class
  -> upstream cmajor_cpp_dsp adapter and impulse architecture
  -> .ir trace
  -> filesCompare against the canonical C++ reference
```

The first integration step is an explicit external `make cmajor` target, not a
member of the default `all` or scheduling matrix. It requires `cmaj` and the
pinned C++ checkout, whereas ordinary workspace and impulse lanes must remain
self-contained apart from their already documented reference dependency.
`CMAJ_BIN`, `CMAJ_CXX`, `CPP_TESTS`, and `FAUST_ARCH` must be overridable.

Adaptations from upstream are deliberate:

- every DSP owns `build/cmajor/<name>/cmajordsp.cmajor`, patch, generated
  header, and executable, avoiding the shared temporary names that make the C++
  recipe unsafe under `make -j`;
- the C++ checkout's `archs/impulsecmajor.cpp`, `controlTools.h`, and
  `architecture/faust/dsp/cmajor-cpp-dsp.h` are referenced in place, consistent
  with the existing native impulse lanes; they are not copied into faust-rs;
- `control.dsp` remains an explicit exclusion initially, matching upstream,
  because that fixture exercises the legacy `control` primitive rather than
  the Cmajor event-control contract;
- the scalar double target is the mandatory first gate. `-fp` and `-mapp`
  variants are added only when the faust-rs CLI accepts and implements those
  options; vector/scheduler matrix entries remain forbidden for scalar Cmajor;
- traces use the same `filesCompare`, per-DSP tolerances, and canonical
  `reference/*.ir` files as the other executable backends.

The impulse lane validates audio outputs and input control events through the
upstream adapter. It does not by itself validate output-event delivery for
bargraphs, because `cmajor_cpp_dsp::compute` copies audio streams but does not
drain Cmajor output events. The separate C6 bargraph cadence runtime test
therefore remains mandatory.

If a pinned reference case exceeds these preliminary thresholds, Phase C0
must record a specific measured tolerance and its cause. Do not introduce one
global loose epsilon.

Run a representative stateful subset with `cmaj -O0` and `cmaj -O4`; include a
delay/recurrence, table lookup, UI-controlled DSP, and bargraph DSP. This guards
against optimization-induced semantic drift.

### 9.5 Minimum fixture matrix

| Fixture | Structural | C++ diff | Cmajor parse | Runtime | Precision |
|---|---:|---:|---:|---:|---|
| identity, 0/1/many channels | yes | yes | yes | yes | f32/f64 |
| arithmetic and comparisons | yes | yes | yes | yes | f32/f64 |
| recurrence and fixed delay | yes | selected | yes | yes | f32/f64 |
| variable delay/dynamic index | yes | yes | yes | yes | f32 |
| forward/reverse loops | yes | selected | yes | yes | f32 |
| full UI widget set | yes | yes | yes | event runtime | f32/f64 |
| bargraphs | yes | yes | yes | cadence runtime | f32/f64 |
| table 64 and 65,536 | yes | yes | yes | yes | f32/f64 |
| two table sizes, same generator | yes | yes | yes | yes | f32 |
| waveform | yes | selected | yes | yes | f32 |
| math map | yes | selected | yes | yes | f32/f64 |
| FAD/RAD/OD/US/DS scalar lowering | yes | selected | yes | yes | representative |
| soundfile/foreign/bitcast/vector | typed reject | yes | n/a | n/a | n/a |

Clock-domain features belong in the scalar Cmajor corpus once their canonical
FIR lowering is available. They do not require a Cmajor vector backend: the
backend must correctly emit the scalar loops and state already present in FIR.

## 10. CI and dependency policy

Use three explicit test capabilities:

| Capability | Required where | Failure policy |
|---|---|---|
| pure Rust Cmajor emitter tests | all normal CI jobs | mandatory |
| C++ differential | dedicated parity job with pinned Faust | mandatory in that job, skipped elsewhere with a clear reason |
| Cmajor frontend/runtime | dedicated job with pinned `cmaj` | mandatory in that job, skipped elsewhere with a clear reason |

Never make ordinary workspace tests depend on `/usr/local/bin/cmaj`, an
installed Faust, or `/usr/local/share/faust`. Discovery must use an explicit
environment variable or a checked tool lookup, and a dedicated job must fail
if the tool it promises is missing.

Before any implementation commit touching `codegen`, `transform`, `compiler`,
or another compilation-pipeline crate, run:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p xtask -- golden-check
cargo run --release -p xtask -- compile-budget-check
```

Run focused Cmajor tests first during development, but do not substitute them
for the full gates before a commit.

## 11. Risks and mitigations

| Risk | Failure mode | Mitigation |
|---|---|---|
| C++ lifecycle exception copied literally | direct `instanceInit` unexpectedly rebuilds tables; repository lifecycle violation | adapted lifecycle plus order-sensitive structural/runtime test |
| table helper type lacks actual length | Cmajor rejects generated function or selects wrong table | FIR inspection in C0; deterministic table plan; multi-size fixture |
| C++ `DeclareFun` no-op copied blindly | helper is referenced but never emitted | inventory function ownership by FIR section; reject unresolved calls |
| endpoint order normalized away | wrappers bind wrong ports or event names | exact endpoint-order contract tests; narrow diff normalizer |
| execution flags treated as optional | block body or in-loop control recomputation emitted | intrinsic capability row; four-shape byte-identity test |
| vector request silently downgraded | user receives scalar code after asking for vector | named capability rejection before lowering |
| bargraph declaration tested without cadence | meters flood or never update | sample-rate-aware runtime event-count test |
| metadata fresh IDs leak between requests | nondeterministic source and cross-request contamination | request-local allocator; repeated/parallel compile tests |
| current Cmajor changes syntax | CI becomes dependent on an unpinned local install | pin tested versions; frontend gate separate from pure Rust tests |
| architecture wrapper remains C++-named | duplicated wrapper code or Cmajor incorrectly rejected by CLI | generalize marker processor with cross-language tests |
| poly wrapper assumptions hidden | missing `freq`, `gain`, `gate`, stereo outputs causes invalid graph | defer and validate explicit endpoint/arity contract in C7 |
| broad source snapshots give false confidence | text looks similar while numbers differ | layered structural, parser, and runtime tests |
| compile-time regression remains green functionally | large DSPs become unusable | release compile-budget gate; no unexplained baseline increase |

## 12. Completion checklist

The scalar Cmajor backend may be marked implemented only when all of these are
true:

- [x] C0 Go decision and the reference pin are recorded.
- [x] `generate_cmajor_module` is a documented, typed public codegen API.
- [x] `-lang cmajor` is reachable through facade and CLI.
- [x] external control and one-sample modes are intrinsic and tested.
- [ ] vector/scheduler/OpenMP and other unsupported modes fail explicitly.
- [x] `float32` and `float64` source is accepted by the pinned Cmajor frontend.
- [x] scalar FIR, state, delay, loop, math, waveform, table, and subcontainer
      fixtures pass.
- [ ] UI endpoint and bargraph contracts pass structurally and at runtime.
- [x] backend lifecycle conformance passes before any golden/impulse enrollment.
- [x] C++ differential differences are classified and narrow.
- [x] Cmajor `-O0`/`-O4` numeric parity passes on the recursive stateful probe.
- [x] the 125-case supported impulse corpus meets its recorded thresholds;
      eight explicit exclusions remain tracked in `known.mk`.
- [x] compiler and codegen README/API documentation is updated.
- [x] `JOURNAL.md`'s daily target records mapping statuses, reference pins,
      tests, known gaps, and any deferred variants.
- [ ] full format, clippy, workspace tests, golden checks, and release
      compile-budget check pass.
- [x] a concise `porting/HANDOFF.md` records branch, HEAD, validation, and next
      Cmajor milestone at the end of each substantial implementation session.
