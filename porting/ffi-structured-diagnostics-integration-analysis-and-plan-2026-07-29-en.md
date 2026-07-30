# Structured Diagnostics Across WASM, Interpreter, and Cranelift FFI: Analysis and Improvement Plan

**Date:** 2026-07-29

**Status:** Proposed; analysis and planning only

**`faust-rs` baseline:** `b5478ed4` (`Stop building type warnings the caller discards`)

**`faustwasm` baseline:** branch `rust` at `74a5132` (`Use renamed faust-rs WASM module`)

**Target repositories:** `faust-rs`, `faustwasm`

**Target components:** `diagnostics`, `compiler`, `wasm-ffi`, `interp-ffi`,
`cranelift-ffi`, their C/C++ headers, `RustLibFaust`, `FaustCompiler`, and the
public TypeScript declarations

**Naming assumption:** the request mentions `faustasm`. No separate local
`faustasm` repository or component was found. This plan treats that name as a
reference to `faustwasm`. If it denotes a distinct project, its API must be
inventoried during P0 before its integration is specified or implemented.

## 1. Executive conclusion

The Rust-backed WASM, Interpreter, and Cranelift integrations all detect a
failed DSP compilation. None of them, however, exposes the complete
`faust-rs` diagnostic model through its public host API.

All three boundaries currently reduce typed compiler failures to strings:

```text
CompilerError
  + DiagnosticBundle
  + SourceMap
        |
        | error.to_string()
        v
  +----------------------+-------------------------+
  |                      |                         |
  v                      v                         v
wasm-ffi              interp-ffi              cranelift-ffi
Err(String)           Result<_, String>        Result<_, String>
  |                      |                         |
  v                      v                         v
JS Error(message)     4096-byte C buffer       4096-byte C buffer
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
compiler-library service and is carried by none of the three FFI layers.

`getErrorDiagnostics()` must return the complete retained diagnostics-v2
report. It deliberately takes no rendering or verbosity parameter: consumers
such as LLMs, IDEs, and UIs can inspect or filter typed fields locally without
losing trace frames, related diagnostics, or debug evidence at the FFI
boundary. This choice must not change DSP acceptance, compiler passes,
diagnostic collection, generated code, factory cache identity, or warning
policy.

The recommended architecture is:

1. move diagnostics-v2 rendering into a shared library API;
2. preserve `CompilerError` and `DiagnosticBundle` until each FFI boundary has
   captured them;
3. introduce an owned diagnostic record that renders the complete JSON report
   on demand;
4. add backend-specific result/query APIs for WASM, Interpreter, and
   Cranelift while keeping their existing text APIs unchanged;
5. expose `getErrorDiagnostics()` in `faustwasm` and equivalent C++
   wrappers;
6. keep successful factory diagnostics, especially warnings, separate from
   failed factory-construction diagnostics.

There is one necessary correction to the proposed factory-level shape: a
factory method alone cannot report a factory-creation failure, because the
existing C APIs return `null` and therefore create no factory object on
failure. A sound API needs a build-result object or an explicit diagnostics
out-parameter for construction errors. A factory-level
`getDiagnostics()` remains useful after successful construction for
warnings and remarks. Returning a non-null "failed factory" is rejected because
it would violate existing lifecycle and cache contracts.

This design gives C, C++, JavaScript, LLM, and tooling consumers a common
machine channel without breaking applications that only display the existing
error string.

## 2. Scope

This plan covers failures produced while the Rust compiler creates WASM,
Interpreter, or Cranelift artifacts and factories:

```text
source decoding
  -> FFI argument decoding
  -> parse/import
  -> eval
  -> propagate
  -> type/interval
  -> transform
  -> FIR verification
  -> backend (WASM / Interpreter / Cranelift)
  -> FFI result or C/C++ factory construction
  -> optional JavaScript factory construction
```

It covers:

- the pure Rust diagnostic serializer needed by non-CLI callers;
- the `wasm-ffi` result-handle ABI;
- the `interp-ffi` and `cranelift-ffi` C ABI constructors and factory wrappers;
- the Interpreter and Cranelift C++ wrapper headers;
- complete diagnostics-v2 reports without FFI-side verbosity selection;
- failed-construction diagnostics versus diagnostics retained by a successful
  factory;
- the raw Rust compiler adapter in `faustwasm`;
- the high-level `FaustCompiler` rejection contract;
- TypeScript types, compatibility, tests, and documentation;
- the distinction between compiler failures and host-side
  `WebAssembly.compile(...)` failures.

It does not authorize implementation. In particular, the external TypeScript
API, raw WASM ABI, C constructor-result ABI, C++ wrapper method, ownership, and
threading choices must be confirmed before implementation because they affect
external compatibility surfaces.

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
- return a non-null invalid factory when compilation fails;
- store the last error in an unsynchronized process-global string;
- make warnings fatal.

Warnings and remarks retained by a successful factory are included as an
additive factory-query goal, but warning production remains opt-in and cannot
change the success/failure decision.

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

### 4.4 `interp-ffi` discards the bundle and truncates the presentation channel

The Interpreter file/string constructors eventually call compiler facade
methods returning `Result<_, CompilerError>`, but
`compile_factory_from_*_fastlane(...)` immediately applies:

```rust
.map_err(|e| format!("{e}"))?
```

The common constructor then writes that string into the historical
caller-provided error buffer and returns `null`. The buffer contract is fixed
at 4096 bytes, so it is unsuitable for a diagnostics-v2 document even if the
document were rendered before the write.

`InterpreterDspFactory` contains only the compiled FBC factory. It retains no
diagnostic bundle, warning bundle, source map, or render context. The C and C++
headers expose the text buffer and successful-factory JSON, but no diagnostic
result or query function.

### 4.5 `cranelift-ffi` loses diagnostics at two different boundaries

The Cranelift source constructors first compile to FIR through `Compiler`, then
invoke the Cranelift JIT backend directly. Both failure families become
`String`:

- `CompilerError` from source-to-FIR is mapped with `e.to_string()`;
- `CraneliftBackendError` from FIR-to-JIT is mapped with `e.to_string()`.

The second conversion is especially important: a complete backend diagnostic
requires the FIR origins and source map that are available during lowering.
The current `BoxFfiFirModule` handoff keeps the FIR store/module and arities but
not a diagnostic record suitable for later rendering.

As with the Interpreter backend, failed constructors write a 4096-byte error
buffer and return `null`. `CraneliftDspFactory` retains runtime and compilation
metadata but no diagnostic bundle. The C++ wrapper's existing
`getWarningMessages()` returns the legacy warning-list surface, not
diagnostics-v2 JSON.

### 4.6 A factory method cannot retrieve a failed construction

The proposed shape:

```text
factory->getErrorDiagnostics()
```

cannot be the only error API under current semantics. When source compilation
fails, both C APIs return a null factory pointer and the C++ wrapper is never
constructed. Calling a method would be impossible and manufacturing a failed
factory would contaminate cache, instance, and lifecycle invariants.

There are four possible transports:

1. a backend-global "last diagnostics" value;
2. a thread-local "last diagnostics" value;
3. a new diagnostics out-parameter on constructor variants;
4. an owned factory-build result handle containing either a factory or failure.

Process-global state is rejected because concurrent factory creation can return
another thread's error. Thread-local state is workable but implicit, fragile
across language runtimes, and hard to compose. An out-parameter is simple but
renders one chosen level eagerly and proliferates constructor signatures.

An owned build-result handle is the recommended primary design: it is explicit,
concurrency-safe, retains the typed bundle, and can render the complete report
on demand. Existing constructors remain compatibility wrappers over that new
internal/result path.

A successful factory may separately expose:

```text
factory->getDiagnostics()
```

for warnings and remarks retained during successful compilation. It should not
be named `getErrorDiagnostics` unless it is explicitly documented to return an
empty report on every successful factory.

### 4.7 `faustwasm` correctly propagates the remaining string

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

### 4.8 The JSON channel is CLI-only

The CLI has a clean diagnostics-v2 channel selected by:

```text
--error-format json
```

It emits one schema-v2 document with compiler metadata, request metadata,
sources, status, and structured diagnostics. The published contract is
`docs/diagnostics-v2.schema.json`.

The serializer currently lives under `crates/compiler/src/cli`, while
all FFI crates link the compiler library. A library caller cannot reuse the
binary-only module without first moving the machine serializer to an
appropriate library boundary.

### 4.9 `--error-format json` cannot activate JSON in an FFI backend

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

### 4.10 The FFI must not inherit CLI verbosity shortcuts

The CLI may retain its own human-output verbosity choices, but the FFI must
not project them into a lossy machine contract. The reusable serializer needed
by this plan must emit every retained diagnostics-v2 field, including debug
evidence and all trace and related-diagnostic entries.

### 4.11 Three different JSON payloads must remain distinct

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
- preserve the existing non-empty human-readable message in JavaScript and C
  error buffers;
- make the schema-v2 diagnostic report available without parsing that message;
- preserve all structured fields carried by `DiagnosticBundle`;
- do so for WASM, Interpreter, and Cranelift compiler failures;
- preserve Cranelift backend diagnostics after source-to-FIR succeeds but JIT
  generation fails;
- render the complete retained diagnostic without recompiling the DSP;
- keep diagnostic presentation out of factory cache keys and compiler option
  strings;
- distinguish a compiler diagnostic from a malformed FFI request and from a
  host `WebAssembly.compile(...)` failure;
- keep result payloads valid until the result handle is freed;
- keep C/C++ diagnostic strings owned until explicitly freed;
- provide an explicit construction-error carrier even when no factory exists;
- optionally retain successful warnings/remarks on a created factory;
- work in browser, worker, and Node environments;
- support source imports and virtual sources without losing their diagnostic
  provenance;
- allow clients to feature-detect the richer ABI when loading an older raw Rust
  compiler module.

### 5.2 Compatibility requirements

The first implementation must be additive:

- existing `faust_wasm_result_error_ptr/len` exports remain;
- their lifetime remains tied to the result handle;
- existing Interpreter and Cranelift constructor signatures, null-on-failure
  behavior, and 4096-byte error buffers remain;
- existing factory cache and deletion semantics remain;
- existing C and C++ headers remain source-compatible;
- `createMonoDSPFactory` and `createPolyDSPFactory` keep their signatures;
- `getErrorMessage()` keeps returning a string;
- caught values remain instances of JavaScript `Error`;
- the historical Emscripten/C++ path remains usable;
- diagnostics-v2 stays at schema version 2;
- successful `{ wasm, dsp_json, compile_options }` results are unchanged.

No consumer should be forced to understand diagnostics JSON merely to display
an error. New constructor-result APIs may be added, but existing entry points
must remain compatibility wrappers rather than silently changing ownership.

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

The Rust, C, C++, raw WASM, and TypeScript queries take no diagnostic-level
argument. Each query returns the complete diagnostics-v2 report retained for
that compilation: all labels, facts, traces, fixes, notes, help, typed debug
context, IR references, and related diagnostics. Source-text inclusion remains
governed by the separate source-text policy and is not implied by the
completeness of the report.

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

**Decision:** superseded by Option E.

### Option D — factory method plus backend/thread-local last error

Add `factory->getErrorDiagnostics()` and use a global or thread-local
record when factory creation returns null.

**Advantages**

- resembles an ordinary factory query;
- can be added with little change to existing constructor signatures.

**Problems**

- the factory method is unavailable for the failure that prevented the factory;
- process-global state is racy;
- thread-local state has implicit lifetime and call-order rules;
- language runtimes may resume error handling on a different thread;
- a later factory call can overwrite the record before the caller reads it;
- helper operations and multiple backends need separate hidden slots.

**Decision:** reject as the primary contract. A thread-local compatibility
helper may be considered only if an existing downstream ABI cannot adopt the
result-handle API.

### Option E — owned diagnostic record and factory-build result handles

Preserve the typed information:

```text
FfiDiagnosticRecord {
    message: String,
    diagnostics: Option<DiagnosticBundle>,
    request: DiagnosticRequestContext,
    source_text_policy: SourceTextPolicy,
}
```

Render the complete JSON report only when the host calls the query.

For `wasm-ffi`, the existing compile-result handle can own the record. Add a
query returning an existing text-result handle:

```text
faust_wasm_result_get_error_diagnostics(result_handle)
    -> text_result_handle
```

For Interpreter and Cranelift, add backend-prefixed factory-build result handles
with conceptual operations:

```text
create...FactoryResultFromFile/String(...)
factory_result_is_ok(result)
factory_result_error_message(result)
factory_result_get_error_diagnostics(result)
factory_result_take_factory(result)
factory_result_free(result)
```

The exact exported symbol spellings and whether the message uses borrowed
pointer/length or an owned C string are ABI decisions for P0. The semantic
requirements are fixed:

- the result owns the failure record;
- on success it owns one factory cache reference until `take_factory`;
- `take_factory` transfers that reference exactly once;
- freeing an untaken successful result releases the reference;
- the diagnostics query can be called more than once and is deterministic;
- diagnostics strings use explicit ownership and are never limited to 4096
  bytes;
- existing constructors call the new internal path, copy the compatibility
  message to the old buffer, and preserve null-on-failure behavior.

**Advantages**

- backwards compatible;
- machine and human channels stay separate;
- no prose parsing;
- exposes all retained structured information without recompilation;
- explicit lifetime and concurrency semantics;
- works when no factory exists;
- older modules can be feature-detected;
- transport/argument errors can keep `diagnostics = None`;
- one internal model serves WASM, Interpreter, and Cranelift.

**Cost**

- additive raw WASM and C APIs, C++ wrapper types, and TypeScript types;
- result/factory transfer semantics must be tested carefully;
- requires moving the serializer out of the CLI-only module.

**Decision:** recommended.

## 7. Proposed public behavior

### 7.1 Common query behavior

Every structured query returns the complete diagnostics-v2 JSON report.
Rendering is deterministic: querying the same retained record twice returns
byte-identical JSON and never recompiles the source.

Compiler failures return `status: "failed"`. Successful-factory queries return
`status: "success"` and contain warnings/remarks only; an empty diagnostic
array is valid.

Transport errors without a `CompilerError` return no compiler diagnostics.
Their human message remains available through the transport's compatibility
error channel.

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

### 7.3 Interpreter and Cranelift C behavior

Add opaque, backend-prefixed factory-build result types. A source constructor
using the new API returns a non-null result handle for every decodable request,
including compilation failure. The caller then:

1. checks `result_is_ok`;
2. on success, takes the factory exactly once;
3. on failure, reads the compatibility message and calls
   `result_get_error_diagnostics(result)`;
4. frees every owned string and the result handle.

The old constructors remain and still return a factory or null. Internally they
use the same build path, copy at most 4095 bytes into `error_msg`, and release
the temporary result. They do not gain hidden global state.

For a successful factory, optional backend-prefixed C queries expose retained
warnings/remarks:

```text
getCInterpreterDSPFactoryDiagnostics(factory)
getCCraneliftDSPFactoryDiagnostics(factory)
```

Each returns an owned C string freed with the backend's existing `freeCMemory`.
These functions do not report the error that prevented a factory from being
created; that error belongs to the build result.

### 7.4 C++ wrapper behavior

Add RAII factory-build result wrappers. Conceptual usage:

```cpp
auto result = createInterpreterDSPFactoryResultFromString(...);
if (!result) {
    std::cerr << result.getErrorMessage();
    auto json = result.getErrorDiagnostics();
}
auto* factory = result.takeFactory();
```

The same shape applies to Cranelift. Exact naming must follow the established
free-function style of the existing headers and be approved in P0.

Successful C++ factories may expose:

```cpp
std::string getDiagnostics();
```

This returns warnings/remarks retained during the successful build. The plan
does not add `getErrorDiagnostics` as an ordinary instance method because an
ordinary instance necessarily represents a successful construction.

### 7.5 TypeScript and `faustwasm` behavior

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

### 7.6 Activation model

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

1. Record current WASM, Interpreter C/C++, and Cranelift C/C++ behavior for:
   - parser error;
   - missing import;
   - undefined symbol/eval error;
   - Box connection/propagate error;
   - type or interval error;
   - transform/FIR error;
   - backend-specific Interpreter, Cranelift, and WASM error;
   - malformed raw FFI input;
   - host rejection of invalid returned WASM bytes.
2. Capture null/factory return, the 4096-byte C buffer, C++ `std::string`,
   JavaScript `Error.message`, cache state, and raw handle lifetime.
3. Measure complete-report payload sizes on representative failures, including
   virtual imports, before finalising the JavaScript ownership strategy.
4. Confirm Option E, exact C symbols, result/factory ownership transfer,
   C++ RAII names, and TypeScript error methods with maintainers.
5. Confirm whether `faustasm` means `faustwasm` or a separate integration.

**Gate:** no external ABI implementation starts until the compatibility,
ownership, threading, and naming decisions are explicit and the baseline
fixtures are committed.

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

### P2 — Preserve typed failures through backend build paths

1. Add an internal owned `FfiDiagnosticRecord` or equivalent outside the
   backend-specific ABI code.
2. Change Interpreter compile helpers to return a typed build error until the
   common FFI boundary instead of applying `format!("{e}")`.
3. Change Cranelift preflight to preserve:
   - `CompilerError` from source-to-FIR;
   - FIR/source origins needed for backend diagnostics;
   - typed `CraneliftBackendError` from JIT generation.
4. Map non-compiler transport, bitcode, allocation, and host-symbol failures
   explicitly without fabricating `FRS-*` diagnostics.
5. Add a compiler artifact/result shape that can retain successful
   warnings/remarks for a factory without making them fatal.

**Gate:** unit tests prove that parse through backend failures still expose the
original `DiagnosticBundle` immediately before each FFI transport renders it.

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

### P4 — Add Interpreter factory-build results and queries

1. Add an opaque Interpreter factory-build result in Rust and the C header.
2. Add file/string result constructors and result inspection/free functions.
3. Define single-transfer `take_factory` behavior over the cache reference.
4. Implement `get...FactoryResultErrorDiagnostics(result)` using
   owned C strings, not the 4096-byte buffer.
5. Keep existing constructors as wrappers with unchanged signatures.
6. Retain successful warnings/remarks in `InterpreterDspFactory` when enabled
   and expose `getCInterpreterDSPFactoryDiagnostics(factory)`.
7. Regenerate or verify the cbindgen header and add C header smoke tests.

**Gate:** failure results work without a factory pointer; taking a factory
twice is rejected; freeing an untaken success releases exactly one cache
reference; old constructors remain source/binary compatible.

### P5 — Add Cranelift factory-build results and queries

Repeat P4 for Cranelift, with additional tests that distinguish:

- compiler front-end/FIR failures;
- Cranelift backend/JIT failures with preserved detail code and origins;
- invalid optimization or foreign-symbol transport failures;
- source, signal, box, and bitcode constructor families where applicable.

Update both manually maintained Cranelift headers and their C/C++ smoke tests.

**Gate:** Cranelift-specific backend evidence survives into JSON, and result
ownership remains correct under cache hits, misses, and concurrent builds.

### P6 — Add C++ RAII APIs

1. Add backend-specific RAII build-result wrappers.
2. Expose `getErrorMessage()`, `getErrorDiagnostics()`, `takeFactory()`,
   and boolean success inspection on the result.
3. Add successful-factory `getDiagnostics()` for retained warnings.
4. Keep all existing free factory-creation functions and wrappers unchanged.

**Gate:** C++ header-smoke programs include both backend headers, exercise
success/failure ownership, and pass on Linux, macOS, and Windows toolchains.

### P7 — Integrate complete typed failures in `faustwasm`

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

### P8 — Add cross-FFI end-to-end failure tests

Build the actual `libfaust-rs.wasm`, load it through the `faustwasm` adapter,
build the Interpreter and Cranelift C/C++ libraries, and compile the same
invalid DSP corpus through every boundary. Do not limit validation to Rust unit
tests of internal registries.

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
- C diagnostic strings and result handles are released;
- Interpreter and Cranelift return the same front-end code/range for the same
  invalid source;
- backend-specific failures identify the selected backend and detail code;
- old C constructors still report their compatibility string and null factory;
- result-handle constructors preserve full, untruncated JSON beyond 4096 bytes;
- the C++ path still rejects using its existing text contract;
- a host `WebAssembly.compile` error has no fabricated DSP diagnostic.

Add at least one browser or headless-browser test because the adapter has
different base64, memory, and module-loading paths in Node and browsers.

**Gate:** Node, browser, C/C++ header smoke, Interpreter/Cranelift runtime tests,
TypeScript build, workspace tests, Clippy, and compiler-module export verifier
are green on all CI platforms.

### P9 — Document the consumer contract

Document:

- how to catch `FaustCompilerError`;
- that `getErrorDiagnostics()` returns the complete report for client-side
  inspection and filtering;
- how to use Interpreter and Cranelift factory-build results from C and C++;
- why construction failures belong to a build result rather than a null
  factory;
- how successful factories expose warnings/remarks;
- why clients must not parse `Error.message`;
- why `--error-format json` is a CLI option, not a `faustwasm` compile option;
- the difference between DSP JSON, auxiliary-files JSON, and diagnostics JSON;
- compatibility behavior for old Rust modules and the C++ compiler;
- source-text/privacy policy;
- schema-version and unknown-field handling;
- a short LLM-oriented example using code, primary range, facts, and fixes.

**Gate:** public C, C++, and TypeScript examples compile in their respective
test/build pipelines and use only exported APIs.

## 9. Improvement backlog

The following improvements are ordered after the core failed-compilation path.

### High priority

1. **One reusable complete-report renderer.**
   Make every FFI boundary return the same complete diagnostics projection;
   CLI display verbosity remains a separate presentation concern.
2. **Typed failure preservation.**
   Remove premature `CompilerError`/backend-error stringification in all three
   FFI paths.
3. **Interpreter and Cranelift build-result handles.**
   Expose construction errors without requiring a factory that does not exist.
4. **Cranelift backend provenance.**
   Preserve FIR/source origins through direct JIT generation so backend
   diagnostics retain their source labels and detail codes.
5. **Structured compile failures in raw WASM and TypeScript.**
   Add the parameter-free query and `FaustCompilerError` methods.
6. **C/C++ compatibility wrappers.**
   Keep old constructors while adding explicit result ownership and RAII.
7. **Correct request metadata.**
   Populate `mode`, `backend`, and `normalized_options` for WASM, Interpreter,
   and Cranelift instead of using CLI placeholders.
8. **Source-text policy.**
   Avoid repeating embedded standard libraries or proprietary virtual sources
   in browser logs by default.
9. **Cross-version capability detection.**
   Allow a new `faustwasm` package to load an older raw Rust compiler module and
   degrade to text-only reporting.

### Medium priority

10. **Warnings on successful factories.**
    Retain opted-in warnings/remarks and expose them through
    `factory.getDiagnostics()` with `status: "success"`.
11. **Rich human message generation.**
   Optionally use the shared human renderer for `Error.message`, while retaining
   the machine payload as authoritative and documenting text instability.
12. **Auxiliary helper failures.**
   Carry structured diagnostics through `expandDSP` and
   auxiliary-file generation in all three backend families, whose helper paths
   currently also stringify service/compiler errors.
13. **Schema-derived TypeScript types.**
   Generate or verify TypeScript declarations from
   `diagnostics-v2.schema.json` to prevent manual drift.
14. **Consumer helpers.**
    Provide small helpers for finding the primary label and machine-applicable
    edits without hiding the underlying report.

### Lower priority

15. **Historical C++ compiler normalization.**
    Wrap historical C++ text errors in a backend-neutral JavaScript error type,
    but do not invent Rust diagnostic codes or source ranges.
16. **Successful diagnostic telemetry.**
    Expose diagnostic counts without transferring full payloads when a UI only
    needs a badge.
17. **Protocol projections.**
    Add LSP, SARIF, or MCP adapters only when a concrete integration needs them;
    they should project diagnostics v2 rather than create another error model.

## 10. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Existing applications depend on exact error text | Preserve current message accessors and signatures; make the structured channel additive |
| JSON is confused with DSP metadata JSON | Use names containing `diagnostics`; document all three JSON contracts |
| New `faustwasm` loads an old compiler module | Feature-detect optional exports and fall back to text |
| Diagnostic JSON parsing fails in JS | Preserve the original message and make the query return `null` |
| Source text or embedded libraries leak into logs | Explicit source-text policy; `faustwasm` default `none` |
| Factory creation fails, so no factory method can be called | Put construction diagnostics on an owned build result, not a failed factory |
| Factory reference is leaked or released twice | Single-transfer `take_factory`; free-result tests for taken and untaken success |
| Result handle is freed before views are copied | Explicit result/text lifetimes; adapters copy views before final free |
| Concurrent builds overwrite "last diagnostics" | Error/result objects own diagnostics; compiler-level last-error query is convenience only |
| A consumer wants a compact view | Consumers filter the complete typed report locally; the FFI does not discard retained fields |
| C error buffer truncates JSON | Keep buffer for text compatibility only; return allocated JSON/result handles |
| Complete report payload is too expensive | Measure P0; require explicit truncation metadata before imposing any cap |
| Serializer drifts among CLI and FFI backends | One shared library serializer and cross-backend schema snapshots |
| Transport errors are mislabeled as DSP errors | Keep diagnostics optional; do not fabricate `FRS-*` reports |
| Structured payload becomes unbounded | Measure P0 corpus sizes; require explicit truncation metadata before imposing any cap |
| Front-end diagnostics differ by backend | Differentially compile the same invalid corpus through WASM, Interpreter, and Cranelift |
| Cranelift direct JIT errors lose origins | Retain FIR origins until backend error mapping completes |
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
| existing Interpreter constructors | `1:1` preserved | same signatures, buffer, null-on-failure, and cache ownership |
| Interpreter build-result/query API | additive Rust extension | explicit error/factory ownership and complete-report query |
| existing Cranelift constructors | `1:1` preserved | same signatures, buffer, null-on-failure, and cache ownership |
| Cranelift build-result/query API | additive Rust extension | explicit error/factory ownership and complete-report query |
| successful factory diagnostic query | additive extension | warnings/remarks only; empty success report allowed |
| C++ build-result wrappers | additive adapted API | RAII over new C handles; old free functions preserved |
| `RustLibFaust.fail` | adapted | throws typed error carrying the complete report |
| `FaustCompiler.getErrorMessage()` | `1:1` preserved | unchanged string compatibility surface |
| `getErrorDiagnostics()` | additive extension | complete report, with no FFI-side filtering |
| factory creation methods | `1:1` preserved | same parameters and Promise result/rejection model |
| historical C++ compiler path | deferred structured mapping | text behavior preserved; no synthetic Rust codes |
| generated WASM, FBC, JIT code, and DSP JSON | `1:1` preserved | no output or runtime change |

## 12. Validation commands

Expected Rust-side gates:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p xtask -- diagnostics-quality-check
cargo run -p xtask -- golden-check
cargo run -p xtask -- build-faustwasm-compiler-module
cargo test -p wasm-ffi -p interp-ffi -p cranelift-ffi
```

Expected `faustwasm` gates:

```bash
npm run build
npm run lint
node <structured-diagnostics-node-test>
```

The final end-to-end gate must use the WASM module and native FFI libraries
produced from the same `faust-rs` commit under test. A passing CLI JSON test
alone is insufficient, because the defect addressed by this plan is
specifically at compiler-library/FFI/host boundaries.

## 13. Completion criteria

This plan is complete when:

1. an invalid DSP compiled through WASM, Interpreter, or Cranelift reports a
   useful compatibility message and no factory/artifact success;
2. every backend exposes the same schema-valid front-end diagnostics-v2 report
   for the same source failure;
3. each retained error returns every retained structured field without
   recompiling;
4. an LLM can identify the stable code, primary edit range, typed facts, trace,
   and applicable fixes without parsing prose;
5. Interpreter and Cranelift construction failures are queryable without a
   factory pointer, through leak-free result handles;
6. successful factories can expose opted-in warnings/remarks independently of
   construction errors;
7. old Rust compiler modules, old native constructors, and the historical C++
   compiler still fail gracefully with text-only errors;
8. raw WASM, C, C++, and JavaScript result/string handles have tested,
   leak-free lifetimes;
9. CLI JSON standard output and schema compatibility remain unchanged;
10. generated WASM, FBC, JIT code, DSP JSON, cache keys, and successful factory
    behavior remain unchanged;
11. source inclusion is explicit and independent of client-side filtering;
12. all Rust workspace, compiler-module, native header/runtime, TypeScript,
    Node, browser, and cross-platform CI gates are green.
