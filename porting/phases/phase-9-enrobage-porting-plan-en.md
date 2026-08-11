# Phase 9 Enrobage Porting Plan (C++ -> Rust)

## 1. Goal

Port C++ `enrobage` functionality needed by the production compile/output flow,
with parity-first behavior and explicit scope boundaries.

C++ source of truth:
- `/Users/letz/Developpements/RUST/faust/compiler/parser/enrobage.hh`
- `/Users/letz/Developpements/RUST/faust/compiler/parser/enrobage.cpp`

## 2. Scope Decision

In scope now:
- architecture/template file search and opening:
  - `openArchStream`
  - `fopenSearch`
- stream copy/wrapping helpers:
  - `streamCopyLicense`
  - `streamCopyUntil`
  - `streamCopyUntilEnd`
- path/output helpers:
  - `fileBasename`
  - `fileDirname`
  - `stripEnd`
  - `makeOutputFile`
- include injection and class-name replacement behavior used during architecture wrapping.

Out of scope in this plan:
- network-backed URL fetch behavior from `checkURL` via `sourcefetcher`
  (`http_fetch` path);
- broad `sourcefetcher` porting, now specified but not implemented in
  [`sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md`](../sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md).

## 3. Target Rust Placement

Recommended placement:
- `crates/compiler/src/enrobage.rs`:
  orchestration-facing wrapping pipeline and stream copy logic.
- `crates/compiler/src/enrobage/search.rs` (or local module):
  architecture/import file search behavior and path assembly.
- optional pure helpers can be promoted to `crates/utils` after parity is stable.

Rationale:
- `enrobage` is output orchestration, not parser-core grammar logic.
- keeps parser crate focused on syntax and import expansion only.

## 4. API Mapping Plan

| C++ API | Rust status target | Notes |
|---|---|---|
| `openArchStream` | `adapted` | return `Result<File, Error>` with deterministic search order |
| `fopenSearch` | `adapted` | preserve fullpath capture + import dir enrichment semantics |
| `streamCopyLicense` | `1:1` | preserve exception-tag behavior for header stripping |
| `streamCopyUntil` | `1:1` | preserve sentinel stop behavior + include injection policy |
| `streamCopyUntilEnd` | `1:1` | thin wrapper around `streamCopyUntil` |
| `fileBasename`/`fileDirname`/`stripEnd` | `1:1` | keep edge-case behavior (root/no-dir/windows-style path) |
| `makeOutputFile` | `adapted` | use `PathBuf` composition while preserving output naming behavior |
| `checkURL` (file/http/https) | `planned` | local-file checks exist; the shared policy-gated native fetcher and remote behavior are specified but not implemented |

## 5. Execution Steps

### Step A: Baseline and Fixtures

- Build parity fixtures from C++ behavior:
  - architecture files with/without license exception tags,
  - include patterns (`#include <faust/...>` and Julia include forms),
  - class-name replacement cases (`mydsp`, `dsp` word-boundary behavior),
  - path edge cases (absolute/relative, nested dirs).
- Deliverable:
  - fixture corpus under `tests/corpus/` or `crates/compiler/tests/fixtures/`.

### Step B: Pure Helpers Port

- Implement and test:
  - basename/dirname/strip/make-output helpers.
- Focus:
  - exact edge behavior parity first, then internal cleanup.
- Deliverable:
  - unit tests in `crates/compiler/tests/enrobage_paths.rs`.

### Step C: Search/Open Semantics Port

- Implement:
  - architecture search order parity for `openArchStream`,
  - `fopenSearch`-equivalent behavior with fullpath return and import-dir enrichment.
- Deliverable:
  - tests for search precedence and fullpath/import-dir side effects.

### Step D: Stream Copy/Injection Port

- Implement:
  - `streamCopyLicense` header removal logic,
  - `streamCopyUntil` and `streamCopyUntilEnd`,
  - include injection + class-name replacement behavior.
- Deliverable:
  - golden text tests against captured C++ outputs.

### Step E: Compiler Integration

- Integrate Rust enrobage module into compile output assembly path.
- Keep integration behind explicit config switch during transition if needed.
- Deliverable:
  - production path calls Rust enrobage wrapper stage.

### Step F: Differential Validation

- Compare Rust and C++ wrapped outputs on selected architecture fixtures.
- Require no untriaged output mismatch.
- Deliverable:
  - parity report artifact:
    - `porting/phases/phase-9-enrobage-diff-report-en.md`.

## 6. Validation Gates

Required checks per step:
- `cargo fmt --all`
- `cargo clippy --workspace --all-targets -- -D warnings`
- targeted tests:
  - `cargo test -p compiler enrobage`
- integration regression:
  - `cargo test -p compiler --no-fail-fast`

For differential step:
- run C++ reference wrapping and Rust wrapping on the same fixtures,
- document any residual mismatch with owner and follow-up date.

## 7. Exit Criteria

The enrobage port can be marked complete when:
- in-scope APIs above are implemented and wired in production path,
- output parity fixtures pass with no untriaged mismatch,
- documentation is updated:
  - `phase-3-parser-adjacent-modules-status-en.md` (enrobage no longer deferred),
  - `phase-9-integration-en.md` integration status updated,
  - `JOURNAL.md` records behavior and parity evidence.

## 8. Execution Status (2026-02-19)

- Step A: completed (`3835350`)
- Step B: completed (`ad5b7d4`)
- Step C: completed (`53141d6`)
- Step D: completed (`5ff437d`)
- Step E: completed (`db919f0`)
- Step F: completed (current branch tip)
