# Phase 3 Parser-Adjacent Modules Status (`SourceFetcher`, `Enrobage`)

## 1. Purpose

This document closes **Gate B remaining step 7** by making lifecycle/API status explicit for parser-adjacent C++ modules that are not part of core grammar migration.

Source of truth (C++):
- `/Users/letz/Developpements/RUST/faust/compiler/parser/sourcefetcher.hh`
- `/Users/letz/Developpements/RUST/faust/compiler/parser/sourcefetcher.cpp`
- `/Users/letz/Developpements/RUST/faust/compiler/parser/enrobage.hh`
- `/Users/letz/Developpements/RUST/faust/compiler/parser/enrobage.cpp`

## 2. Status Matrix (`1:1` / `adapted` / `deferred`)

| C++ module | Main C++ API / role | Rust target scope | Status | Rationale | Owner + milestone | Validation |
|---|---|---|---|---|---|---|
| `sourcefetcher` | low-level `http_fetch(...)` and HTTP helpers used by import/file handling | optional, policy-gated native transport injected into parser source resolution | `planned` (implementation not started) | The dependency, source-identity, security, disabled-network, and cross-target contracts are now specified; core parser/import functionality remains local-file based until the plan lands. | Parser/compiler integration track, target **Phase 9 integration**; see [`sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md`](../sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md) | Existing local/cycle behavior remains the baseline; implementation must add hermetic feature-on/off and local HTTP differential coverage. |
| `enrobage` | architecture-template/file helper set (`openArchStream`, `fopenSearch`, stream copy utilities, output naming) used by `libcode.cpp` and documentator | `compiler` integration layer (`crates/compiler/src/enrobage.rs`) | `adapted` (implemented for C++ output path) | Implemented in Rust with parity-first stream/path helpers and explicit CLI integration (`-a/-A/-i`) for C++ output. Remaining work is full end-to-end output parity cleanup outside strict enrobage scope (codegen-header differences). | Compiler/codegen integration track, **Phase 9 implemented milestone** (report: `phase-9-enrobage-diff-report-en.md`) | `compiler` enrobage tests pass: `enrobage_paths`, `enrobage_search`, `enrobage_stream`, `enrobage_integration`; wrapper differential checks documented in Phase 9 report. |

## 3. Scope Contract for Phase 3

- `SourceReader` in `parser` remains **local-file only** until the approved
  implementation plan lands.
- URL/network imports are still not fetched by the current implementation.
- No placeholder network/wrapper implementation is introduced in Phase 3.
- This is an explicit **defer decision**, not an omission.

## 4. Integration Preconditions for Phase 9

Before implementing the planned network-source module:
1. Define feature policy for remote fetch (default-off, reproducible/offline-safe behavior).
2. Place APIs at the right boundary (`compiler`/`codegen`/`doc` orchestration layer) instead of parser-core.
3. Add lifecycle mapping per API (`1:1` or `adapted`) with compatibility impact notes.
4. Add focused tests for:
   - successful/failed URL fetch cases (if enabled),
   - wrapper/architecture file insertion behavior parity,
   - deterministic behavior when network is disabled.

The decisions and executable phase breakdown for these preconditions are now
recorded in
[`sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md`](../sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md).

## 5. Step-5 Coverage Update (Import Envelope)

Additional parser-side `SourceReader` / API tests now cover:
- local-directory import precedence over global search paths when both provide the same import name,
- parent-relative import resolution (`../...`) through nested source trees,
- uniqueness of `used_files` tracking under repeated imports through different paths.

These checks extend the local-file import parity envelope while keeping network fetch out of scope for Phase 3.
