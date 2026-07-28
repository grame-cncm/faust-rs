# Phase 0 — Validation Sprint (Go/No-Go)

> **Scope**: pre-port validation and scope freeze before implementation
> **Estimate**: 5–10 person days
> **Prerequisites**: none (must happen before Phase 1)

---

## 1. Objectives

Phase 0 validates high-risk assumptions on the current C++ branch so the Rust port starts from a stable baseline.

Primary goals:
- Confirm the effective production pipeline to port first.
- Freeze migration scope (included and excluded backends/modes).
- Validate API lifecycle model and integration constraints.
- Clarify public API migration policy (`1:1` vs adapted) and traceability.
- Confirm and enforce the local quality gate used by all porting steps (`fmt` + `clippy -D warnings` + tests).
- Lock differential testing baseline and acceptance thresholds.
- Enforce stub minimization in validation prototypes.
- Define and enforce source-provenance documentation in Rustdoc format during porting.
- Require explicit deliverables and pass criteria per validation task before execution.
- Freeze a shared structured diagnostics model baseline for parser/eval/propagate/compiler.
- Surface blockers early with explicit go/no-go criteria.

---

## 2. Mandatory validation work

### 2.1 Effective pipeline confirmation
- Confirm that major end-to-end paths are still driven by `libcode.cpp` dispatch + `InstructionsCompiler` / `DAGInstructionsCompiler`.
- Keep `signalFIRCompiler` and `signalRenderer` as maintained but non-critical/experimental tracks.
- Record exact call paths used by `-lang c`, `-lang cpp`, `-lang rust`, `-lang wasm`, `-lang llvm`, `-lang interp`.

### 2.2 Scope freeze
- Confirm that `backend-java` is out of Rust port target scope.
- Confirm that old C++ mode `-lang ocpp` is out of Rust port target scope.
- Keep any legacy references only as historical context, not as deliverable requirements.

### 2.3 CLI/backend option model
- Inventory backend/option compatibility rules currently spread across imperative checks.
- Define a declarative capability matrix target for Rust.
- Add contradiction tests (for example impossible or duplicated conditions).

### 2.4 API lifecycle and ownership model
- Inventory all C/C++ entry points and their context init/teardown behavior.
- Identify divergent lifecycle paths and normalize to one Rust contract.
- Lock explicit ownership rules for compile session handles and returned artifacts.
- For `box_signal_api.cpp`, limit Phase 0 to usage/surface inventory; defer full export prioritization to Phase 1 scope planning.

### 2.4.b Public API mapping policy
- Define the rule for API migration status on touched public surfaces:
  - `1:1`: same conceptual API with equivalent contract,
  - `adapted`: Rust-idiomatic signature/model with preserved documented semantics,
  - `deferred`: not yet ported, with owner and milestone.
- Require a mapping record for touched APIs including:
  - C++ symbol/file reference,
  - Rust equivalent,
  - status (`1:1` / `adapted` / `deferred`),
  - compatibility impact and validation tests.

### 2.5 Orchestration safety checks
- Validate that no fixed-size temporary argument staging is kept in Rust design.
- Validate deterministic per-request state reset in orchestration paths.
- Validate that output mode handling is typed (text vs binary capabilities), not stream-downcast-based.

### 2.6 Stack/recursion strategy
- Audit recursive hotspots and current stack workaround behavior.
- Define iterative or bounded-recursion strategy where needed.
- Record maximum supported recursion/depth behavior for parity and diagnostics.

### 2.7 Differential baseline
- Build representative DSP corpus (small, medium, large, pathological).
- Capture baseline outputs on selected backends.
- Define tolerated differences (formatting/cosmetic) and non-tolerated differences (semantic/structural/runtime).

### 2.8 TreeArena hash-consing performance validation
- Implement a minimal Rust `TreeArena` prototype (`make`, intern lookup, traversal, property access).
- Run micro-benchmarks for creation, lookup on existing nodes, traversal, and property set/get.
- Compare against equivalent C++ `CTree` benchmark on the same workload profile.
- Record thresholds and optimization levers (hash function choice, pre-allocation, map implementation).

### 2.9 Stub minimization discipline
- Prototype through real crate APIs and real data paths whenever feasible.
- Avoid parser-local or pass-local placeholder layers that duplicate target crate responsibilities.
- If a stub is unavoidable, document:
  - technical reason,
  - explicit owner,
  - removal milestone within the same phase gate.
- Add a "stub debt" tracker entry to prevent carry-over into subsequent phases.

### 2.10 Source-provenance documentation discipline
- Define a Rustdoc convention for migrated code (`///` / `//!`) with:
  - source C++ file/function reference,
  - parity-sensitive invariants and behavior notes.
- Apply the convention in all Phase 0 prototypes touching core migration risks.
- Record exceptions in `JOURNAL.md` when provenance cannot be attached immediately (owner + follow-up milestone).

### 2.11 Quality gate discipline
- Standardize the mandatory local validation commands used throughout the port:
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace --all-targets`
- Define exception handling when one command cannot run in a given step:
  - explicit reason,
  - owner,
  - target date for completion,
  - tracking entry in `JOURNAL.md`.

### 2.12 Diagnostics model baseline
- Define a shared diagnostics schema (`severity`, `stage`, `code`, source labels, notes/help) in `diagnostics`.
- Validate at least one parser error and one downstream error class (`eval` or `propagate`) converted to the schema.
- Freeze renderer requirements (human + JSON) and deterministic snapshot policy.
- Reference architecture and staged rollout document:
  - `porting/faust-rust-diagnostics-model-en.md`.

---

## 3. Deliverables

- `phase-0-validation-en.md` updated with:
  - confirmed pipeline map
  - `gGlobal` decomposition map for critical flow (`phase-0-gglobal-decomposition-map-en.md`)
  - scope decisions
  - capability matrix draft
  - API lifecycle contract
  - Public API mapping policy and status convention (`1:1` / `adapted` / `deferred`)
  - baseline differential protocol
  - Rustdoc source-provenance convention for migrated modules
  - TreeArena performance report (results, thresholds, decision) (`phase-0-treearena-benchmark-report-en.md`)
  - diagnostics model baseline (schema + code taxonomy + renderer contract) (`porting/faust-rust-diagnostics-model-en.md`)
- Go/No-Go decision with explicit blockers list.
- If blockers exist: mitigation plan with owner and target phase.

---

## 4. Go/No-Go criteria

Go:
- Effective production pipeline is confirmed and documented.
- Scope exclusions are frozen (`backend-java`, `-lang ocpp`).
- API lifecycle model is unified and accepted.
- Public API mapping policy is defined and applied on touched Phase 0 surfaces.
- Capability matrix approach is defined and testable.
- Differential baseline corpus and procedure are ready.
- Rustdoc source-provenance convention is defined and applied on touched prototypes.
- TreeArena hash-consing performance is validated against agreed thresholds.
- No untracked stubs remain in Phase 0 prototypes.
- Diagnostics schema baseline is validated across parser and one downstream phase.

No-Go:
- Pipeline ownership is ambiguous between competing compile paths.
- Scope is not frozen or remains contradictory across docs.
- API lifecycle remains divergent with no agreed target contract.
- Public API mapping remains implicit (no `1:1`/`adapted`/`deferred` status on touched APIs).
- Differential baseline cannot be reproduced reliably.
- Source-provenance remains implicit (no Rustdoc convention and no tracked exceptions).
- TreeArena performance is outside agreed limits with no credible mitigation path.
- Stub-dependent prototypes hide target architecture boundaries or carry unowned stub debt.
- Diagnostics remain fragmented in ad hoc string channels with no shared schema/code taxonomy.

---

## 5. Exit checklist

- [ ] Pipeline target confirmed (`InstructionsCompiler` path first)
- [ ] Experimental tracks documented (`signalFIRCompiler`, `signalRenderer`)
- [ ] Java backend excluded from target scope
- [ ] `-lang ocpp` excluded from target scope
- [ ] Capability matrix model defined for CLI/backend validation
- [ ] `gGlobal` decomposition map drafted and reviewed for touched flows
- [ ] API lifecycle unified across entry points
- [ ] Public API mapping policy defined and used (`1:1` / `adapted` / `deferred`)
- [ ] No fixed-size temporary argument staging in target design
- [ ] Deterministic per-request orchestration state model documented
- [ ] Typed output sink model documented
- [ ] Recursion/stack strategy documented
- [ ] Differential corpus and acceptance rules finalized
- [ ] Rustdoc source-provenance convention defined and applied on touched prototypes
- [ ] Quality gate command set finalized and enforced (`fmt` + `clippy -D warnings` + tests)
- [ ] TreeArena hash-consing benchmarks completed and reviewed
- [ ] Stub debt tracker reviewed (no unowned/unplanned carry-over)
- [ ] Diagnostics model baseline validated (schema + stable codes + human/JSON outputs)
- [ ] Final Go/No-Go decision recorded
