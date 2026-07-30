# Structured Diagnostics Across the WASM FFI: Analysis and Improvement Plan

**Date:** 2026-07-29

**Status:** Implementation in progress; P0 contract frozen on 2026-07-30

**`faust-rs` baseline:** `b5478ed4` (`Stop building type warnings the caller discards`)

**`faustwasm` baseline:** branch `rust` at `74a5132` (`Use renamed faust-rs WASM module`)

**Target repositories:** `faust-rs`, `faustwasm`

**Target components:** `diagnostics`, `compiler`, `wasm-ffi`, `RustLibFaust`,
`FaustCompiler`, and the public TypeScript declarations

**Scope note:** this is the WASM-only version of
`ffi-structured-diagnostics-integration-analysis-and-plan-2026-07-29-en.md`.
The Interpreter and Cranelift FFI boundaries are deliberately out of scope
here; that document remains the reference if they are taken up later.

**Naming assumption:** the request mentions `faustasm`. No separate local
`faustasm` repository or component was found. This plan treats that name as a
reference to `faustwasm`. If it denotes a distinct project, its API must be
inventoried during P0 before its integration is specified or implemented.

## 0. Implementation progress and frozen P0 decisions

The implementation request of 2026-07-30 approves Option D and freezes these
external compatibility decisions:

- the raw export is
  `faust_wasm_result_get_error_diagnostics(result_handle)`;
- it returns an independently owned text-result handle;
- the query takes no diagnostic-level or presentation argument and always
  returns the complete retained diagnostics-v2 report;
- existing error pointer/length accessors and compile-result lifetimes remain
  unchanged;
- `FaustCompilerError.getErrorDiagnostics()` is the authoritative
  per-failure TypeScript API;
- `FaustCompiler.getErrorDiagnostics()` is a last-error convenience only;
- FFI reports omit source text by default while preserving source metadata,
  hashes, and ranges;
- `faustasm` is treated as the `faustwasm` integration named by this document.

The baseline is `faust-rs` commit `b5478ed4` and `faustwasm` branch `rust` at
`74a5132`. Existing compile-result and text-result lifetime tests are the
compatibility baseline; phase-specific negative fixtures are added with the
implementation that makes their structured assertions possible.

Implementation progress:

- P0 complete: contract frozen in `22434645`;
- P1 complete: reusable complete diagnostics-v2 renderer;
- P2–P6 pending.

## 1. Executive conclusion

The Rust-backed WASM integration detects a failed DSP compilation. It does not,
however, expose the complete `faust-rs` diagnostic model through its public
host API.

The boundary currently reduces typed compiler failures to a string:

```text
CompilerError
  + DiagnosticBundle
  + SourceMap
        |
        | error.to_string()
        v
     wasm-ffi
     Err(String)
        |
        v
  JS Error(message)
```

The host therefore learns that compilation failed, but can lose the
information that makes the new diagnostics useful:

- stable `FRS-*` code;
- compiler stage and ownership category;
- primary and related source ranges;
- typed semantic facts;
- binding, import, evaluation, and transformation traces;
- suggested fixes and their applicability;
- source hashes and immutable source snapshots;
- backend detail codes and compiler-bug classification.

The diagnostics-v2 JSON format already exists and is validated for the CLI,
but it is implemented in the binary-only CLI module. It is not yet a reusable
compiler-library service and is not carried by the FFI layer.

`getErrorDiagnostics()` must return the complete retained diagnostics-v2
report. It deliberately takes no rendering or verbosity parameter: consumers
such as LLMs, IDEs, and UIs can inspect or filter typed fields locally without
losing trace frames, related diagnostics, or debug evidence at the FFI
boundary. This choice must not change DSP acceptance, compiler passes,
diagnostic collection, generated code, factory cache identity, or warning
policy.

The recommended architecture is:

1. move diagnostics-v2 rendering into a shared library API;
2. preserve `CompilerError` and `DiagnosticBundle` until the FFI boundary has
   captured them;
3. introduce an owned diagnostic record that renders the complete JSON report
   on demand;
4. add a diagnostics query on the existing WASM result handle while keeping its
   text API unchanged;
5. expose `getErrorDiagnostics()` in `faustwasm`;
6. keep diagnostics retained by a successful compilation, especially warnings,
   separate from failure diagnostics.

The WASM boundary is in a comparatively good position for this work: it already
returns an owned result handle for every request, successful or not, so a
failure has somewhere to live without inventing a new ownership model. The
missing pieces are the retained typed record and the query that renders it.

This design gives JavaScript, LLM, and tooling consumers a machine channel
without breaking applications that only display the existing error string.

## 2. Scope

This plan covers failures produced while the Rust compiler creates WASM
artifacts and factories:

```text
source decoding
  -> FFI argument decoding
  -> parse/import
  -> eval
  -> propagate
  -> type/interval
  -> transform
  -> FIR verification
  -> WASM backend
  -> FFI result
  -> optional JavaScript factory construction
```

It covers:

- the pure Rust diagnostic serializer needed by non-CLI callers;
- the `wasm-ffi` result-handle ABI;
- complete diagnostics-v2 reports without FFI-side verbosity selection;
- failure diagnostics versus diagnostics retained by a successful compile;
- the raw Rust compiler adapter in `faustwasm`;
- the high-level `FaustCompiler` rejection contract;
- TypeScript types, compatibility, tests, and documentation;
- the distinction between compiler failures and host-side
  `WebAssembly.compile(...)` failures.

It does not authorize implementation. In particular, the external TypeScript
API, the raw WASM ABI, and ownership and threading choices must be confirmed
before implementation because they affect external compatibility surfaces.

## 3. Non-goals

This plan does not:

- change DSP acceptance or generated WASM/JSON output;
- replace the diagnostics-v2 schema;
- put machine JSON inside `Error.message`;
- reinterpret `--error-format` as a DSP backend option;
- emulate the Emscripten filesystem or C++ exception machinery;
- require the historical C++ compiler path to synthesize Rust `FRS-*` codes;
- add SARIF, LSP, MCP, or IDE protocols;
- expose raw Box, Signal, or FIR dumps to an LLM;
- change the precompiled-artifact mode;
- store the last error in unsynchronized module-global state;
- make warnings fatal;
- extend the Interpreter or Cranelift FFI boundaries.

Warnings and remarks retained by a successful compile are included as an
additive query goal, but warning production remains opt-in and cannot change
the success/failure decision.

## 4. Current-state analysis

### 4.1 The compiler still owns the complete diagnostic

Every `CompilerError` variant carries a `DiagnosticBundle`.
`CompilerError::diagnostic_bundle()` is exhaustive, so adding an error variant
without a bundle is a compile-time failure rather than a silent fallback.

For a parse failure, for example, the bundle can contain the exact parser
message, stable code, source range, source snapshot, and repair information.
This data survives through the compiler facade and is available when
`Compiler::compile_wasm_artifact(...)` returns `Err`.

### 4.2 `Display` is not a diagnostic renderer

`CompilerError::fmt` is an operational summary. For a parse error it reports:

```text
parse failed for <source>: errors=<n>, recoveries=<n>, diagnostics=<n>
```

That summary is suitable for an error chain or debug log, but not for a DSP
author, IDE, or LLM. It does not include the actual syntax error, stable code,
label, fix, or trace.

Other variants may include a more descriptive inner error, but none of them
are required to reproduce the structured bundle. Consumers must not infer
machine meaning by parsing `Display` text.

### 4.3 `wasm-ffi` discards the bundle

`compile_to_stored_result(...)` currently maps:

```rust
Err(error) => StoredCompileResult::Err(error.to_string())
```

`StoredCompileResult::Err` owns only one `String`. Once this conversion has
happened, neither the raw WASM host nor `faustwasm` can recover the diagnostic
bundle.

The ABI exports only:

- result success/failure;
- WASM bytes on success;
- DSP JSON on success;
- compile options on success;
- one error string on failure.

There is no diagnostics payload pointer/length pair and no capability query for
one.

### 4.4 `faustwasm` correctly propagates the remaining string

On a failed result handle, `RustLibFaust.createDSPFactory(...)`:

1. reads `faust_wasm_result_error_ptr/len`;
2. frees the result handle;
3. stores the text in `fLastError`;
4. throws `new Error(message)`.

`FaustCompiler.createDSPFactory(...)` catches that exception, copies
`getErrorAfterException()` into `fErrorMessage`, calls cleanup, and rethrows an
`Error` containing the same message.

This lifecycle is correct for the information the ABI currently exposes. The
problem is information loss before JavaScript receives the handle, not an
exception-swallowing bug in TypeScript.

### 4.5 The JSON channel is CLI-only

The CLI has a clean diagnostics-v2 channel selected by:

```text
--error-format json
```

It emits one schema-v2 document with compiler metadata, request metadata,
sources, status, and structured diagnostics. The published contract is
`docs/diagnostics-v2.schema.json`.

The serializer currently lives under `crates/compiler/src/cli`, while the FFI
crate links the compiler library. A library caller cannot reuse the binary-only
module without first moving the machine serializer to an appropriate library
boundary.

### 4.6 `--error-format json` cannot activate JSON in the WASM backend

The string passed to `createDSPFactory(name, code, args, ...)` is a Faust
compilation-option string. `ffi-common` parses only its supported compile
subset and ignores unknown options.

Consequently:

```text
createMonoDSPFactory(name, code, "--error-format json")
```

still returns the current text-only error. Adding recognition of that flag to
the compile-option parser would mix presentation policy with backend semantics
and would remain awkward for typed JavaScript callers.

Diagnostic retrieval should instead be a parameter-free host API. The FFI
returns the complete retained report, while each host chooses its own display
or filtering policy.

### 4.7 The FFI must not inherit CLI verbosity shortcuts

The CLI may retain its own human-output verbosity choices, but the FFI must
not project them into a lossy machine contract. The reusable serializer needed
by this plan must emit every retained diagnostics-v2 field, including debug
evidence and all trace and related-diagnostic entries.

### 4.8 Three different JSON payloads must remain distinct

The integration already uses JSON for unrelated contracts:

| Payload | Purpose | Success/failure |
|---|---|---|
| DSP JSON | UI, metadata, inputs/outputs, runtime layout | successful compile |
| auxiliary-files JSON | transport for generated SVG and other artifacts | helper success |
| diagnostics-v2 JSON | explanation of compilation diagnostics | compile failure, and later optional warnings |

Naming APIs merely `json` or `getJSON()` would make these contracts easy to
confuse. New names must include `diagnostics`.

## 5. Requirements

### 5.1 Functional requirements

The completed integration must:

- reject invalid DSP source as it does today;
- preserve the existing non-empty human-readable message in JavaScript;
- make the schema-v2 diagnostic report available without parsing that message;
- preserve all structured fields carried by `DiagnosticBundle`;
- preserve WASM backend diagnostics after the front end succeeds but code
  generation fails;
- render the complete retained diagnostic without recompiling the DSP;
- keep diagnostic presentation out of factory cache keys and compiler option
  strings;
- distinguish a compiler diagnostic from a malformed FFI request and from a
  host `WebAssembly.compile(...)` failure;
- keep result payloads valid until the result handle is freed;
- optionally retain warnings/remarks from a successful compile;
- work in browser, worker, and Node environments;
- support source imports and virtual sources without losing their diagnostic
  provenance;
- allow clients to feature-detect the richer ABI when loading an older raw Rust
  compiler module.

### 5.2 Compatibility requirements

The first implementation must be additive:

- existing `faust_wasm_result_error_ptr/len` exports remain;
- their lifetime remains tied to the result handle;
- existing factory cache and deletion semantics remain;
- `createMonoDSPFactory` and `createPolyDSPFactory` keep their signatures;
- `getErrorMessage()` keeps returning a string;
- caught values remain instances of JavaScript `Error`;
- the historical Emscripten/C++ path remains usable;
- diagnostics-v2 stays at schema version 2;
- successful `{ wasm, dsp_json, compile_options }` results are unchanged.

No consumer should be forced to understand diagnostics JSON merely to display
an error. New queries may be added, but existing entry points must remain
compatibility wrappers rather than silently changing ownership.

### 5.3 Machine-consumer requirements

LLMs and tools must read typed fields rather than prose:

- `code`, `detail_code`, `category`, and `stage` identify the failure;
- the label with `role: "primary_cause"` identifies the edit location;
- `facts` carries expected/actual values and violated conditions;
- `traces` explains how the compiler reached the failure;
- `fixes[].applicability` controls whether an edit can be automated;
- `sources[].content_hash` detects stale source.

No FFI adapter or wrapper may reconstruct any of these values from
`Error.message`, `notes`, or `help`.

### 5.4 Complete-report requirements

The raw WASM and TypeScript queries take no diagnostic-level argument. Each
query returns the complete diagnostics-v2 report retained for that compilation:
all labels, facts, traces, fixes, notes, help, typed debug context, IR
references, and related diagnostics. Source-text inclusion remains governed by
the separate source-text policy and is not implied by the completeness of the
report.

### 5.5 Payload and privacy requirements

The CLI currently embeds text for memory and virtual-library sources. Repeating
all embedded libraries in every browser error can unnecessarily enlarge the
payload and expose source text to downstream logging.

The shared serializer should therefore accept an explicit source-text policy:

```text
none
primary-memory-source
all-memory-sources
```

Recommended defaults:

- CLI: retain `all-memory-sources` for behavior compatibility;
- `faustwasm`: use `none`, because the caller already owns the submitted DSP;
- diagnostic/debug tools: opt into `primary-memory-source` or
  `all-memory-sources`.

Source names, ranges, kinds, and hashes remain available under every policy.
No silent diagnostic-count truncation should be introduced. If measurement
shows that a cap is needed, the schema must report truncation explicitly.

## 6. Design options

### Option A — replace the error string with JSON

Return diagnostics JSON through the existing error pointer/length accessors.

**Advantages**

- smallest ABI change;
- immediate machine readability.

**Problems**

- breaks applications that display the string directly;
- makes `Error.message` contain a large serialized object;
- conflates transport with presentation;
- leaves no clean representation for failures that have no
  `DiagnosticBundle`.

**Decision:** reject.

### Option B — activate JSON through the Faust argument string

Teach the FFI option parser to recognize `--error-format json`.

**Advantages**

- resembles the CLI;
- no new high-level TypeScript method is strictly required.

**Problems**

- mixes compiler options with binding presentation policy;
- unknown-option behavior makes capability detection unreliable;
- forces string construction where a typed host option is more appropriate;
- a cache key could change even though generated DSP artifacts do not;
- still needs an ABI representation for both text and JSON.

**Decision:** reject as the primary API.

### Option C — eagerly store one JSON level

Store both the compatibility message and one rendered diagnostics JSON string:

```text
StoredCompileFailure {
    message: String,
    diagnostics_json: Option<String>,
}
```

This stores only a rendered string and cannot guarantee that every retained
structured field is available to the caller.

**Decision:** superseded by Option D.

### Option D — owned diagnostic record queried as a complete report

Preserve the typed information on the result handle that already exists:

```text
FfiDiagnosticRecord {
    message: String,
    diagnostics: Option<DiagnosticBundle>,
    request: DiagnosticRequestContext,
    source_text_policy: SourceTextPolicy,
}
```

Render the complete JSON report only when the host calls the query:

```text
faust_wasm_result_get_error_diagnostics(result_handle)
    -> text_result_handle
```

The exact exported symbol spelling is an ABI decision for P0. The semantic
requirements are fixed:

- the result owns the failure record;
- the diagnostics query can be called more than once and is deterministic;
- rendered strings use the existing text-result ownership protocol and are
  never truncated;
- existing error accessors keep their current meaning and lifetime.

**Advantages**

- backwards compatible;
- machine and human channels stay separate;
- no prose parsing;
- exposes all retained structured information without recompilation;
- explicit lifetime semantics reusing the existing handle protocol;
- older modules can be feature-detected;
- transport/argument errors can keep `diagnostics = None`.

**Cost**

- additive raw WASM API and TypeScript types;
- requires moving the serializer out of the CLI-only module.

**Decision:** recommended.

### Option E — module-global "last diagnostics" slot

Store the most recent failure in a module-global value and expose a free
function to read it, instead of attaching it to the result handle.

**Advantages**

- no change to the result-handle ABI;
- superficially convenient for callers that discarded the handle.

**Problems**

- a later compile overwrites the record before the caller reads it;
- helper operations would need their own hidden slots;
- lifetime and call-order rules become implicit;
- it is the one design that cannot express "this specific failure", which is
  exactly what a concurrent or batched caller needs.

**Decision:** reject as the primary contract. A convenience last-error query in
TypeScript (§7.4) is acceptable precisely because the authoritative object is
the rejected error, not the slot.

## 7. Proposed public behavior

### 7.1 Common query behavior

Every structured query returns the complete diagnostics-v2 JSON report.
Rendering is deterministic: querying the same retained record twice returns
byte-identical JSON and never recompiles the source.

Compiler failures return `status: "failed"`. Successful-compile queries return
`status: "success"` and contain warnings/remarks only; an empty diagnostic
array is valid.

Transport errors without a `CompilerError` return no compiler diagnostics.
Their human message remains available through the existing error accessors.

### 7.2 Raw WASM behavior

For a compiler failure:

```text
result_is_ok(handle) == 0
result_error_ptr/len(handle) -> compatibility message
result_get_error_diagnostics(handle) -> text-result handle
```

For an FFI transport failure that cannot produce a `CompilerError`:

```text
result_is_ok(handle) == 0
result_error_ptr/len(handle) -> transport message
result_get_error_diagnostics(handle) -> error/empty text result
```

The diagnostic text-result handle follows the existing
`faust_wasm_text_result_*` lifetime API. The compile-result handle must remain
alive while the diagnostic text result is created, but the returned text handle
then owns its rendered string independently.

For a successful compile, an optional query exposes retained warnings/remarks
on the same handle. It should not be named `get_error_diagnostics` unless it is
explicitly documented to return an empty report on every success.

### 7.3 TypeScript and `faustwasm` behavior

Introduce a public error subtype conceptually equivalent to:

```ts
class FaustCompilerError extends Error {
    getErrorDiagnostics(): FaustDiagnosticReport | null;
    cause?: unknown;
}
```

Recommended behavior:

- Rust compiler failure: reject with `FaustCompilerError` carrying the
  complete retained report;
- old Rust module: reject with `FaustCompilerError` whose diagnostic query
  returns `null`;
- historical C++ compiler failure: preserve its message and use
  `null` diagnostics until that backend exposes an equivalent contract;
- `WebAssembly.compile(...)` failure: preserve the host exception as `cause`
  and do not misclassify it as a DSP diagnostic.

`getErrorMessage()` remains the compatibility accessor. An additive
`FaustCompiler.getErrorDiagnostics()` returns the most recent failure report
for compatibility with code that does not retain the
rejected error object. The rejected `FaustCompilerError` is authoritative for
concurrent compilations because a single compiler-level "last error" slot can
be overwritten.

Raw WASM handles cannot safely remain attached to arbitrary JavaScript errors
without an explicit disposal protocol. The adapter should therefore query and
copy the complete report before freeing the compile result. It must not retain
a raw handle or cache multiple lossy views.

### 7.4 Activation model

Structured diagnostics should be collected automatically on failed Rust
compilations. Successful warnings/remarks are retained only when their existing
opt-in policy enables them. In both cases JSON is rendered only when queried,
so an ordinary successful compile pays no serialization cost.

Clients activate their use by calling:

- `error.getErrorDiagnostics()`, or
- `compiler.getErrorDiagnostics()`.

They should not add `--error-format json` to DSP compiler arguments.

## 8. Implementation plan

### P0 — Freeze the integration baseline and approve the compatibility contract

Before changing an external surface:

1. Record current WASM behavior for:
   - parser error;
   - missing import;
   - undefined symbol/eval error;
   - Box connection/propagate error;
   - type or interval error;
   - transform/FIR error;
   - WASM backend error;
   - malformed raw FFI input;
   - host rejection of invalid returned WASM bytes.
2. Capture `JavaScript Error.message`, cache state, and raw handle lifetime.
3. Measure complete-report payload sizes on representative failures, including
   virtual imports, before finalising the JavaScript ownership strategy.
4. Confirm Option D, the exact exported symbol, and the TypeScript error
   methods with maintainers.
5. Confirm whether `faustasm` means `faustwasm` or a separate integration.

**Gate:** no external ABI implementation starts until the compatibility,
ownership, and naming decisions are explicit and the baseline fixtures are
committed.

### P1 — Make complete diagnostics-v2 rendering a library service

1. Move the machine serializer out of the binary-only CLI module.
2. Expose a documented library API that accepts:
   - `&DiagnosticBundle`;
   - compiler and request metadata;
   - source-text policy.
3. Keep Clap parsing and stdout/stderr policy in the CLI.
4. Make the CLI call the shared serializer for the complete-report path.
5. Preserve schema-v2 field names and existing CLI compatibility output.

**Gate:**

- existing CLI JSON snapshots remain byte-for-byte stable where context is
  unchanged;
- every negative-corpus payload validates against
  `docs/diagnostics-v2.schema.json`;
- `xtask diagnostics-quality-check` passes;
- the library renderer has direct unit tests independent of the CLI binary.

### P2 — Preserve typed failures through the WASM build path

1. Add an internal owned `FfiDiagnosticRecord` or equivalent outside the
   ABI-specific code.
2. Stop applying `error.to_string()` before the FFI boundary; carry the typed
   failure until the transport renders it.
3. Preserve the origins and source map needed for WASM backend diagnostics when
   the front end succeeds and code generation fails.
4. Map non-compiler transport, allocation, and host-symbol failures explicitly
   without fabricating `FRS-*` diagnostics.
5. Add a compile-result shape that can retain successful warnings/remarks
   without making them fatal.

**Gate:** unit tests prove that parse through backend failures still expose the
original `DiagnosticBundle` immediately before the FFI transport renders it.

### P3 — Extend the `wasm-ffi` result contract

1. Replace the internal `Err(String)` storage with a structured failure that
   owns the compatibility message and optional diagnostic record.
2. Add `faust_wasm_result_get_error_diagnostics(handle)` returning
   a text-result handle.
3. Define error behavior for success, transport failures,
   invalid handles, and freed handles.
4. Add the exports to the compiler-module export verification gate.
5. Keep the old error accessors unchanged.

**Gate:**

- parse, import, eval, propagate, type, transform, FIR, and backend failures
  expose a complete schema-valid JSON report;
- transport failures expose a message and no fake compiler diagnostic;
- repeated queries are deterministic and do not recompile;
- compile-result and text-result handles can be freed in either valid order
  after the text result has been created;
- the distributed `libfaust-rs.wasm` contains the required exports.

### P4 — Integrate complete typed failures in `faustwasm`

1. Add TypeScript definitions for the diagnostics-v2 envelope.
2. Mark the new raw export optional during the compatibility transition.
3. Feature-detect it when instantiating a Rust compiler module.
4. Query/copy the complete report before freeing the result handle.
5. Parse diagnostics JSON defensively:
   - require `schema_version`;
   - preserve unknown fields;
   - degrade to `null` if parsing fails;
   - never replace the original message with a JSON parse error.
6. Introduce `FaustCompilerError.getErrorDiagnostics()`.
7. Add `FaustCompiler.getErrorDiagnostics()` as a last-error
   convenience, not the concurrency-safe authoritative object.
8. Preserve `fLastError`, cleanup behavior, `getErrorMessage()`, and factory
   method signatures.

**Gate:**

- existing consumers that only inspect `error.message` behave unchanged;
- new consumers can branch on `error instanceof FaustCompilerError`;
- each query returns the complete retained report;
- an old Rust module and the C++ module still work;
- concurrent rejected compilations retain their own reports;
- no result/text handle remains live after the adapter has copied its views;
- malformed diagnostics JSON cannot hide the original compilation failure.

### P5 — Add end-to-end failure tests

Build the actual `libfaust-rs.wasm`, load it through the `faustwasm` adapter,
and compile an invalid DSP corpus through the boundary. Do not limit validation
to Rust unit tests of internal registries.

Minimum assertions:

- the Promise rejects;
- no factory is cached;
- `Error.message` is non-empty;
- `getErrorDiagnostics().schema_version === 2`;
- `status === "failed"`;
- the expected `FRS-*` code and stage are present;
- the primary label resolves inside the submitted DSP;
- facts, traces, and fixes survive when present;
- imported and virtual-source labels retain correct source ids;
- result handles and request buffers are released;
- backend-specific failures identify the selected backend and detail code;
- the JSON is complete and untruncated;
- the C++ path still rejects using its existing text contract;
- a host `WebAssembly.compile` error has no fabricated DSP diagnostic.

Add at least one browser or headless-browser test because the adapter has
different base64, memory, and module-loading paths in Node and browsers.

**Gate:** Node, browser, TypeScript build, workspace tests, Clippy, and
compiler-module export verifier are green on all CI platforms.

### P6 — Document the consumer contract

Document:

- how to catch `FaustCompilerError`;
- that `getErrorDiagnostics()` returns the complete report for client-side
  inspection and filtering;
- how successful compiles expose warnings/remarks;
- why clients must not parse `Error.message`;
- why `--error-format json` is a CLI option, not a `faustwasm` compile option;
- the difference between DSP JSON, auxiliary-files JSON, and diagnostics JSON;
- compatibility behavior for old Rust modules and the C++ compiler;
- source-text/privacy policy;
- schema-version and unknown-field handling;
- a short LLM-oriented example using code, primary range, facts, and fixes.

**Gate:** public TypeScript examples compile in their respective test/build
pipelines and use only exported APIs.

## 9. Improvement backlog

The following improvements are ordered after the core failed-compilation path.

### High priority

1. **One reusable complete-report renderer.**
   Make the WASM boundary and TypeScript return the same complete diagnostics
   projection. CLI display verbosity remains a separate presentation concern.
2. **Typed failure preservation.**
   Remove premature `CompilerError`/backend-error stringification in the WASM
   path.
3. **Structured compile failures in raw WASM and TypeScript.**
   Add the level query and `FaustCompilerError` methods.
4. **Correct request metadata.**
   Populate `mode`, `backend`, and `normalized_options` for WASM instead of
   using CLI placeholders.
5. **Source-text policy.**
   Avoid repeating embedded standard libraries or proprietary virtual sources
   in browser logs by default.
6. **Cross-version capability detection.**
   Allow a new `faustwasm` package to load an older raw Rust compiler module and
   degrade to text-only reporting.

### Medium priority

7. **Warnings on successful compiles.**
   Retain opted-in warnings/remarks and expose them with `status: "success"`.
8. **Rich human message generation.**
   Optionally use the shared human renderer for `Error.message`, while retaining
   the machine payload as authoritative and documenting text instability.
9. **Auxiliary helper failures.**
   Carry structured diagnostics through `expandDSP` and auxiliary-file
   generation, whose helper paths currently also stringify service/compiler
   errors.
10. **Schema-derived TypeScript types.**
    Generate or verify TypeScript declarations from
    `diagnostics-v2.schema.json` to prevent manual drift.
11. **Consumer helpers.**
    Provide small helpers for finding the primary label and machine-applicable
    edits without hiding the underlying report.

### Lower priority

12. **Historical C++ compiler normalization.**
    Wrap historical C++ text errors in a backend-neutral JavaScript error type,
    but do not invent Rust diagnostic codes or source ranges.
13. **Successful diagnostic telemetry.**
    Expose diagnostic counts without transferring full payloads when a UI only
    needs a badge.
14. **Protocol projections.**
    Add LSP, SARIF, or MCP adapters only when a concrete integration needs them;
    they should project diagnostics v2 rather than create another error model.
15. **Other FFI boundaries.**
    If the Interpreter or Cranelift boundaries are taken up later, they should
    reuse the shared complete-report renderer and the record type
    introduced here rather than growing a second model. Their specific
    ownership problem — a failed constructor returns no object to query — is
    analysed in the full-scope companion document.

## 10. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Existing applications depend on exact error text | Preserve current message accessors and signatures; make the structured channel additive |
| JSON is confused with DSP metadata JSON | Use names containing `diagnostics`; document all three JSON contracts |
| New `faustwasm` loads an old compiler module | Feature-detect optional exports and fall back to text |
| Diagnostic JSON parsing fails in JS | Preserve the original message and make the query return `null` |
| Source text or embedded libraries leak into logs | Explicit source-text policy; `faustwasm` default `none` |
| Result handle is freed before views are copied | Explicit result/text lifetimes; adapters copy views before final free |
| Concurrent builds overwrite "last diagnostics" | The rejected error object owns its diagnostics; the compiler-level last-error query is convenience only |
| A consumer wants a compact view | Consumers filter the complete typed report locally; the FFI does not discard retained fields |
| Complete report payload is too expensive | Measure P0; require explicit truncation metadata before imposing any cap |
| Serializer drifts between CLI and FFI | One shared library serializer and cross-surface schema snapshots |
| Transport errors are mislabeled as DSP errors | Keep diagnostics optional; do not fabricate `FRS-*` reports |
| Structured payload becomes unbounded | Measure P0 corpus sizes; require explicit truncation metadata before imposing any cap |
| WASM backend errors lose origins | Retain origins until backend error mapping completes |
| Historical C++ and Rust compilers expose different detail | Guarantee common `Error` behavior, but document that structured v2 is initially Rust-only |
| Cache behavior changes because of diagnostics | Do not pass diagnostics queries or formatting flags through the compile argument string |

## 11. API mapping and compatibility status

| Surface | Mapping | Planned impact |
|---|---|---|
| `CompilerError::diagnostic_bundle()` | internal `1:1` source | no semantic change |
| diagnostics-v2 JSON | `1:1` schema projection | move to reusable library API; schema unchanged |
| `StoredCompileResult::Err` | adapted internal representation | string becomes message plus optional diagnostic record |
| existing WASM error accessors | `1:1` preserved | no signature or lifetime change |
| WASM diagnostics query | additive Rust extension | parameter-free complete-report text-result handle; optional for older modules |
| successful-compile diagnostic query | additive extension | warnings/remarks only; empty success report allowed |
| `RustLibFaust.fail` | adapted | throws typed error carrying the complete report |
| `FaustCompiler.getErrorMessage()` | `1:1` preserved | unchanged string compatibility surface |
| `getErrorDiagnostics()` | additive extension | complete report, with no FFI-side filtering |
| factory creation methods | `1:1` preserved | same parameters and Promise result/rejection model |
| historical C++ compiler path | deferred structured mapping | text behavior preserved; no synthetic Rust codes |
| generated WASM and DSP JSON | `1:1` preserved | no output or runtime change |

## 12. Validation commands

Expected Rust-side gates:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p xtask -- diagnostics-quality-check
cargo run -p xtask -- golden-check
cargo run -p xtask -- build-faustwasm-compiler-module
cargo test -p wasm-ffi
```

Expected `faustwasm` gates:

```bash
npm run build
npm run lint
node <structured-diagnostics-node-test>
```

The final end-to-end gate must use the WASM module produced from the same
`faust-rs` commit under test. A passing CLI JSON test alone is insufficient,
because the defect addressed by this plan is specifically at the
compiler-library/FFI/host boundary.

## 13. Completion criteria

This plan is complete when:

1. an invalid DSP compiled through WASM reports a useful compatibility message
   and no factory/artifact success;
2. the boundary exposes a schema-valid diagnostics-v2 report for a source
   failure;
3. each retained error returns every retained structured field without
   recompiling;
4. an LLM can identify the stable code, primary edit range, typed facts, trace,
   and applicable fixes without parsing prose;
5. successful compiles can expose opted-in warnings/remarks independently of
   failure diagnostics;
6. old Rust compiler modules and the historical C++ compiler still fail
   gracefully with text-only errors;
7. raw WASM and JavaScript result/string handles have tested, leak-free
   lifetimes;
8. CLI JSON standard output and schema compatibility remain unchanged;
9. generated WASM, DSP JSON, cache keys, and successful compile behavior remain
   unchanged;
10. source inclusion is explicit and independent of client-side filtering;
11. all Rust workspace, compiler-module, TypeScript, Node, browser, and
    cross-platform CI gates are green.
