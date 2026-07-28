# Compiler diagnostics v2 — G4 Signal provenance and typed inference

**Date:** 2026-07-28
**Baseline:** `78b7b01a` (`Track occurrence provenance through Box evaluation`)

## Objective

G4 closes the largest remaining front-end provenance gap: a type or interval
failure used to retain only a formatted internal Signal expression. The new
path carries the source derivation from Box propagation into Signal typing,
keeps the information through the private signal-preparation arena, and emits
typed diagnostic facts.

This is an adapted Rust design. C++ Faust uses mutable Tree properties and
usually formats the current Signal expression in the error string. Rust keeps
semantic identity and diagnostic identity separate.

## Design

### Source-neutral Signal origins

`propagate::SignalOrigins` is an explicit side table:

```text
SigId -> ordered, deduplicated BoxId candidates
```

`propagate_in_slot_env` records the current Box for output signals and for
newly created internal Signal nodes. Origins already assigned by child
propagation calls remain more specific. Hash-consed signals accumulate
candidates; source position never becomes part of `TreeArena` identity.

`SignalCompileOutput` owns this table beside the propagated forest. The
compiler joins it with `parser::BoxProvenance` only while constructing a
diagnostic. Exact occurrences are selected inside the reachable owning
definition; conservative definition/call-site fallbacks remain available.

### Cross-arena and rewrite preservation

`TreeArena::clone_forest_from_with_mapping` returns the arena-local
source-to-destination id map. The provenance-aware preparation entry point,
`prepare_signals_for_fir_verified_with_origins`, uses it to remap origins into
the staging arena.

Preparation then preserves origins across:

- De Bruijn to symbolic recursion conversion;
- unary-recursion projection canonicalization;
- both promotion passes;
- both simplification passes;
- isomorphic recursion merging;
- one-sample delay canonicalization;
- all five retyping points.

New nodes inherit the ordered union of their children. Lane-preserving root
rewrites additionally inherit the replaced root's origins, because an operator
replacement need not retain the old node as a child.

### Typed inference failures

`InferenceError(String)` is removed. `sigtype` now exposes:

- `InferenceError`, with offending `SigId`, relevant operands, inferred
  `SigType`/`Interval`, and required contract;
- `InferenceRule`, with stable machine spellings;
- structured variants for delay bounds, soundfile parts, table construction,
  compile-time math domains, clock-environment misuse, recursive groups, and
  malformed Signal IR.

The compiler preserves `FRS-COMP-0004` because stable top-level diagnostic
codes are not renumbered. Its stage is now explicitly `type_inference`, and
the rule becomes `detail_code`. Actual/required types and intervals are JSON
facts. Signal ids, operand ids, and readable Signal IR are debug context only;
the standard message never exposes `SIG...`.

`CompilerError::Type` boxes `InferenceError`. This is an adapted representation
change required to keep the public aggregate error below Clippy's large-error
threshold after the typed variants gained structured payloads.

## Manual error coverage

The examples from the Faust error manual now have the following Rust behavior:

| Manual family | G4 result |
| --- | --- |
| soundfile part outside `[0,255]` | source label, actual interval object, required integer range |
| invalid/unbounded delay interval | source label, actual full type and interval, explicit bound rule |
| invalid table generator/size | source label, actual type, required initialization/static contract |
| compile-time math-domain failure | source label, operation/domain facts; `% 0`, `/ 0`, `fmod`, `remainder`, `sqrt`, `log`, `log10`, `asin`, and `acos` covered |

Runtime-dependent math domains are not promoted to hard errors. The historical
`-wall -me` warning policy remains a separate renderer/policy task.

## Compatibility and API mapping

| Surface | Mapping | Impact |
| --- | --- | --- |
| Box-to-Signal semantics | `1:1` | generated Signal values are unchanged |
| provenance storage | `adapted` | explicit side table replaces mutable Tree properties |
| `InferenceError` | `adapted` | public tuple struct becomes an inspectable enum |
| `CompilerError::Type.error` | `adapted` | now `Box<InferenceError>`; source compatibility break for direct constructors/patterns |
| `FRS-COMP-0004` | `1:1` stable code | stage changes from `compiler` to `type_inference` |
| preparation without provenance | `1:1` wrapper | existing entry points retain behavior |
| provenance-aware preparation | Rust extension | new explicit entry point |

## Failure modes and mitigation

| Risk | Mitigation |
| --- | --- |
| source location changes Signal equality | origins remain outside `TreeArena` |
| shared signal selects the wrong occurrence | ordered candidate set plus reachable-definition selection |
| staging clone invalidates `SigId` keys | clone returns a complete id map |
| operator rewrite loses parent origin | paired before/after root inheritance |
| internal IR leaks into normal output | IR fields live only in debug context |
| typed error makes aggregate error too large | boxed payload in `CompilerError::Type` |
| runtime math uncertainty becomes fatal | domain failure requires a compile-time (`Konst`) interval wholly outside the domain |

## Structural tests

- propagation retains multiple Box origins for one hash-consed Signal;
- staging remaps origins and retains them through `Delay(_, 1) -> Delay1`;
- soundfile diagnostics expose typed actual/required intervals;
- negative delay diagnostics expose the inferred type and Faust source;
- invalid `rdtable` generator diagnostics expose the table rule and source;
- compile-time modulo-by-zero diagnostics expose the math-domain rule and
  source;
- standard messages contain no raw Signal expression.

## Phase gate

G4 passes when formatting, Rustdoc, Clippy, workspace tests, golden snapshots,
and structural checkers are green. G5 may consume the prepared provenance to
attach the same source chain to FIR and backend failures.
