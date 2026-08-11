# Top-Level `declare` Metadata Global Store Parity Plan

> **Date**: 2026-03-07
> **Scope**: `crates/parser`, `crates/eval`, `crates/compiler`
> **Reference C++ baseline**: `master-dev-ocpp-od-fir-2-FIR19` (`8eebea429`)
> **Reference C++ source roots**:
> - `/Users/letz/Developpements/RUST/faust/compiler/parser/sourcereader.cpp`
> - `/Users/letz/Developpements/RUST/faust/compiler/global.hh`
> - `/Users/letz/Developpements/RUST/faust/compiler/evaluate/eval.cpp`
> **Status**: implemented in parser/eval/compiler with shared session metadata snapshots

## 1. Problem Statement

The parser/pattern/eval parity work is closed for:

- definition-scoped metadata reinjection through `BOXMETADATA`
- parser/eval transport of `declare <def> <key> <value>;`

Top-level:

```faust
declare key "value";
```

had still been handled as an `adapted` Rust representation:

- recorded in `ParserCtx`
- local to the current parse result
- not stored in a compilation-global metadata set

This is now implemented semantically: Rust writes top-level metadata into one
shared compilation-session store that plays the role of `gMetaDataSet`.

## 2. C++ Target Semantics

Relevant reference function:

- `/Users/letz/Developpements/RUST/faust/compiler/parser/sourcereader.cpp`
  - `declareMetadata(Tree key, Tree value)`

Its behavior is:

1. if the declaration appears in the master document:
   - store under plain key `key`
2. if the declaration appears in an imported document:
   - prefix the key with the source filename
   - store under `filename/key`
3. insert the value into the compilation-global metadata set

So the semantic model is:

- one metadata store per compilation session,
- shared across all parsed/imported/loaded sources,
- file-aware key normalization,
- not parser-local bookkeeping only.

## 3. Implemented Rust Behavior

Current Rust evidence:

- `crates/parser/src/context.rs`
  - `declared_metadata`
- `crates/parser/src/lib.rs`
  - `declare_metadata_from_token(...)`

Current behavior:

1. parser sees `declare key "value";`
2. parser still stores `(key, value)` in `ParserCtx` as a local helper view
3. parser also writes the declaration into `CompilationMetadataStore`
4. `ParseOutput` exposes a deterministic metadata snapshot
5. imports / `component(...)` / `library(...)` now converge on the same shared
   top-level metadata state through `EvalSourceContext`
6. `SignalCompileOutput` exposes the final aggregated session snapshot

This keeps parser-local bookkeeping as an adapted helper while moving the real
semantic carrier to a compilation-global session store.

## 4. Why This Matters

This gap affects more than documentation:

- metadata from imports is not normalized and aggregated like C++
- later backend/runtime metadata surfaces cannot rely on one canonical source
  of truth
- file-backed evaluation now happens in `eval`, so top-level metadata semantics
  can drift silently between parser-driven and eval-driven source loading

The problem is therefore architectural:

- the metadata state belongs to the compilation session,
- but Rust currently stores it inside per-parse local state.

## 5. Implemented Rust Design

## 5.1 Compilation-global metadata store

Rust now uses:

- `parser::CompilationMetadataStore`
- `parser::CompilationMetadataSnapshot`
- `parser::CompilationMetadataKey`

Desired semantics:

- one store per compilation session
- shared across parser and eval-driven source loading
- deterministic insertion behavior
- path-aware keys matching the C++ rules

The concrete implementation lives in `crates/parser` so it can be shared by
both `compiler` and `eval` without introducing a dependency cycle.

## 5.2 Parser-local bookkeeping remains a secondary view

`ParserCtx::declared_metadata` can still exist if useful for:

- diagnostics
- parser result inspection
- tests

but it should no longer be the primary semantic carrier.

Mapping status after the port:

- parser-local vector: `adapted helper`
- compilation-global metadata set: `1:1 semantic carrier`

## 5.3 Shared session threading through file loading

The context must be passed through:

- top-level parser entry points
- import expansion
- `eval` file loading for `component(...)`
- `eval` file loading for `library(...)`

Otherwise Rust will still diverge from C++ because different source-loading
paths would update different metadata state.

## 5.4 C++ key-normalization rule

Rust must mirror:

1. master document:
   - `key`
2. imported/loaded document:
   - `filename/key`

Rust now applies this rule structurally through:

- `CompilationMetadataKey::Global { key }`
- `CompilationMetadataKey::Scoped { source_file, key }`

instead of flattening everything into a slash-joined string key.

## 5.5 Stable consumer-facing API

Implemented exposure:

1. `ParseOutput::compilation_metadata`
2. `SignalCompileOutput::compilation_metadata`

Backend/runtime callback consumption can build on that canonical session-level
source later.

## 6. Executed Plan

## Step 1. Define the shared metadata container

Delivered:

- `CompilationMetadataSet` or equivalent
- deterministic insert API
- Rustdoc documenting C++ provenance and key semantics

Status:

- one Rust type exists that can represent the C++ `gMetaDataSet` role for a
  compilation session

## Step 2. Thread it into parser entry points

Delivered:

- parser APIs that can write top-level `declare` entries into the shared store
- parser still returns existing metadata views for diagnostics/tests as needed

Status:

- parsing a master document with `declare key "value";` updates the shared store

## Step 3. Port imported-file prefix semantics

Delivered:

- imported files write `filename/key`
- master document writes plain `key`
- behavior covered by tests

Status:

- Rust matches C++ key naming for both master and imported source files

## Step 4. Unify eval-driven source loading

Delivered:

- `component(...)` and `library(...)` parsing writes into the same metadata
  store as the top-level compiler parse
- no duplicated or isolated metadata state per loader path

Status:

- one compilation session accumulates metadata across parse/import/eval source
  loading paths

## Step 5. Expose or consume the store

Delivered:

- compiler-facing access to aggregated metadata and/or
- backend/runtime callback integration using this canonical source

Status:

- the metadata store is no longer an internal sink with no downstream contract

## 7. Test Plan

Minimum required tests:

1. master document metadata

```faust
declare name "main";
process = _;
```

Expected:

- shared metadata store contains `name -> "main"`

2. imported file metadata

`main.dsp`

```faust
import("lib.dsp");
process = _;
```

`lib.dsp`

```faust
declare author "lib-author";
```

Expected:

- shared metadata store contains `lib.dsp/author -> "lib-author"`

3. mixed master + import metadata

Expected:

- plain master keys remain unprefixed
- imported keys are prefixed
- both coexist in the same compilation-global store

4. eval-driven source loading path

Example:

```faust
process = component("child.dsp");
```

where `child.dsp` contains top-level metadata.

Expected:

- metadata lands in the same shared store used by the enclosing compilation

## 8. Risks And Design Decisions

## 8.1 Ownership boundary

Main design decision:

- should the store be owned by `compiler`,
- or by a smaller shared compilation context used by parser and eval?

Recommendation:

- use a shared context type owned by `compiler` and passed down

This matches the fact that the semantics are compilation-global, but the writes
occur in multiple lower layers.

## 8.2 Deterministic multi-value behavior

The C++ structure is set-like per key. Rust must make this explicit:

- preserve insertion order, or
- canonicalize deterministically

The choice must be documented and tested.

## 8.3 Source path normalization

The meaning of `filename` in `filename/key` must be pinned:

- raw source name,
- resolved file path,
- basename,
- or import-expanded logical name

This must match the C++ observable behavior used as parity reference.

## 9. Current Outcome

This gap is now closed for the parser/eval/compiler scope because:

- top-level `declare key "value";` is written into a compilation-global metadata
  store rather than remaining parser-local only
- imported and eval-loaded sources update the same store
- key prefix rules match the C++ compiler semantically
- the resulting metadata is observable through stable parser/compiler output
  contracts
- metadata wrappers on evaluated definitions update the explicit session store,
  matching C++ `eval.cpp` without process-global state
- the compiler facade transports the normalized snapshot into C and C++
  `metadata()` callbacks; imported keys use logical relative/basename paths and
  callback ordering matches the C++ metadata map
