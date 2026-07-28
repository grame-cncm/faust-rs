# Compiler diagnostics v2 — G5 FIR and backend provenance

**Date:** 2026-07-28

**Baseline:** `06c9c3b3` (`Carry source provenance through Signal typing`)

## Objective

G5 preserves source derivation after Signal preparation so transform, FIR
verifier, and backend failures can identify the Faust construct that produced
the rejected generated node. It also removes the remaining
`fir_code=...`/`codegen_code=...` note protocols in favor of typed v2 fields.

## Design

### FIR provenance side table

`transform::signal_fir::FirOrigins` is an explicit side table:

```text
FirId -> ordered FirSignalOrigin {
    prepared SigId,
    ordered BoxId candidates
}
```

Source position is never inserted into `FirStore`, so FIR hash-consing,
canonical fingerprints, scheduling, and generated code remain unchanged.
When one shared FIR node is produced by several Signals, the table accumulates
all derivations in deterministic Signal-id order.

Scalar and checked-vector lowering record direct Signal producers at their
cache/materialization boundaries. After lifecycle/module assembly, a
post-order pass inherits child derivations into values, statements, blocks,
functions, and the module. This covers scheduling placement, vector
materialization/CSE, lifecycle placement, and module assembly without
instrumenting every builder call.

The mandatory scaffolding sweep clones the FIR store. Its new
`sweep_scaffolding_drop_roots_with_mapping` variant returns a one-to-many
source/destination mapping; `FirOrigins::remap_pairs` transfers provenance and
derives it again over the canonical module.

### Error provenance

`SignalFirError` can retain an offending prepared Signal and resolved Box
candidates. Preparation verification gained a typed `ValidationAt` variant for
the previously string-only offending projection. The compiler joins these Box
ids with occurrence-aware parser provenance and attaches an immutable source
range.

FIR verifier failures carry their existing `FirId`; the compiler now resolves
that id through `FirOrigins` and emits bounded debug evidence plus a Faust
source label. Backend errors expose an optional `fir_node()` through
`BackendCodegenError`. The C++ backend retains the node for unsupported-node
failures. WASM currently lacks precise node capture, so its canonical module
root is a conservative trace anchor; the derived source set remains bounded
and deterministic.

### Typed classification and fields

Backend failures are classified as:

- `unsupported_feature` for a valid FIR construct deliberately outside a
  backend subset;
- `compiler_bug` for malformed roots/sections and other internal invariants.

Unsupported diagnostics recommend another backend or a source rewrite.
Invariant diagnostics explicitly avoid blaming DSP syntax and request a
minimal reproducer.

FIR/backend-local codes now occupy `detail_code` plus typed `fir_code` or
`codegen_code` facts. The legacy same-information notes are removed. Stable
top-level `FRS-FIR-*`, `FRS-SFIR-*`, and `FRS-CODEGEN-0001` codes are
unchanged.

## API mapping

| Surface | Mapping | Compatibility impact |
| --- | --- | --- |
| Signal/FIR semantics | `1:1` | generated FIR and backend text are unchanged |
| `SignalFirOutput::origins` | Rust extension | new public diagnostic side table |
| `FirCompileOutput::origins` | Rust extension | canonical FIR exposes derivations |
| `SignalCompileOutput` diagnostic context | `adapted` | adds definition root and entrypoint name |
| `SignalFirError` provenance | `adapted` | adds optional Signal and Box context |
| `CompilerError::Transform.error` | `adapted` | boxed to keep the aggregate error compact |
| `BackendCodegenError` | `adapted` | adds typed kind and optional `FirId` with defaults |
| scaffolding sweep mapping | Rust extension | old two-value API remains as a wrapper |
| JSON FIR/backend subcodes | v2 replacement | note protocol removed; typed fields are authoritative |

External C/C++ ABI, CLI option acceptance, exit status, and stable top-level
diagnostic codes are unchanged.

## Failure modes and mitigation

| Risk | Mitigation |
| --- | --- |
| provenance changes FIR equality | side table remains outside `FirStore` |
| hash-consed FIR loses occurrence multiplicity | ordered Signal-origin union |
| CSE/materialization creates unanchored parents | derive origins through the final graph |
| canonical clone invalidates ids | explicit one-to-many clone mapping |
| backend reports no exact node | optional node contract; conservative module-root fallback |
| internal invariant blames user source | `compiler_bug` category and report-oriented help |
| debug output grows without bound | only first Signal id is rendered; origin sets are deterministic |

## Structural coverage

- shared FIR nodes accumulate several Signal/Box derivations;
- statement/module parents inherit child derivations;
- canonical compiler FIR retains derivations after scaffolding sweep;
- strict FIR verifier diagnostics carry typed subcodes and Faust labels;
- unsupported SFIR preparation errors point to the originating Faust range;
- backend diagnostics carry typed subcodes, no legacy note protocol, and a
  source trace;
- C++ backend errors distinguish unsupported features from invariants and can
  retain an offending `FirId`.

## Phase gate

G5 passes when formatting, Rustdoc, Clippy, workspace tests, golden snapshots,
and structural checkers are green. G6 can build parser/import recovery on the
same typed v2 envelope without changing the FIR provenance contract.
