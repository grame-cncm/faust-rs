# Compiler diagnostics v2 — G2 typed schema

Date: 2026-07-28

Plan:
`porting/compiler-diagnostics-v2-analysis-and-improvement-plan-2026-07-28-en.md`

## 1. Decision and scope

G2 replaces the former unversioned JSON payload. The project owner explicitly
authorized removing JSON v1: `--error-format json` is now the only machine
diagnostic spelling and emits `schema_version: 2`. There is deliberately no
`json-v2` alias and no compatibility renderer.

This is an intentional pre-stability break for machine consumers. It does not
change diagnostic `FRS-*` meanings, human rendering, process status, source
acceptance, C/C++ or Wasm APIs, generated code, or successful non-diagnostic
CLI output.

## 2. Typed report model

`diagnostics` now owns format-independent machine vocabulary:

- `DiagnosticCategory` separates user code, unsupported features, invalid
  options, environment failures, cancellation, and compiler bugs;
- `DetailCode` provides a pass/backend-local stable discriminator without
  multiplying top-level `FRS-*` codes;
- `FactKey` and `DiagnosticValue` carry deterministic typed facts;
- `LabelRole` distinguishes cause, use, definition, call, operator, import,
  delimiter, expectation, conflict, and derivation sites;
- `DiagnosticTrace` and `TraceFrame` carry ordered causal evidence;
- `SuggestedFix`, `TextEdit`, and `Applicability` describe safe or advisory
  repairs;
- `RelatedDiagnostic` groups non-recursive supporting reports;
- `DebugContext` contains opt-in internal evidence.

All public items have Rustdoc. `Diagnostic` exposes additive builder methods;
`Label::with_role` decouples semantic role from display prose. Deterministic
objects use `BTreeMap`, while traces, labels, fixes, and edits preserve producer
order in `Vec`.

## 3. Source and privacy contract

The envelope serializes the immutable G1 source map. Each source has a stable
session id, kind, name, SHA-256 content hash, and optional text. Memory and
virtual-library sources embed text because no later filesystem lookup can
recover it. File and imported-file entries omit text by default to avoid
duplicating or leaking source files.

Canonical locations are half-open UTF-8 byte `SourceRange` values. Labels also
carry the legacy line/column span while producers migrate, but consumers
should prefer `range`.

## 4. No prose parsing

The v2 serializer reads only typed fields. It does not infer roles from label
messages and does not recover facts/debug values from prefixes in `notes` or
`help`. Human prose remains useful to people, but changing that prose cannot
silently change the machine contract.

Initial producers explicitly tag eval definition/call sites and unresolved
import sites. Later provenance phases will populate more facts, traces, and
fixes at the point where the compiler still owns the necessary semantic data.

## 5. Published schema

- `docs/diagnostics-v2.schema.json` is the normative JSON Schema draft
  2020-12 description.
- `docs/diagnostics-v2-example.json` is a representative failed compilation.
- CLI integration tests parse the complete stdout document, assert the shared
  structural constraints, and run every `tests/corpus/err_*.dsp` entry.

The envelope contains compiler/request metadata, status, sources, and an
ordered diagnostic array. Standard mode emits `debug: null`; debug mode emits
only explicitly provided typed debug fields.

## 6. Public API mapping

Mapping status is `adapted`.

- C++ Faust has no equivalent typed diagnostic report model; Rust adds one
  while preserving compiler semantics.
- Existing Rust `Diagnostic`, `Label`, and `DiagnosticBundle` are extended
  rather than replaced.
- `SourceSpan` remains available for legacy producers, but canonical machine
  coordinates use G1 `SourceRange`.
- The CLI JSON payload intentionally breaks its earlier unversioned shape.
  Consumers must require `schema_version == 2`.

No ABI or IR layout changes are part of G2.

## 7. Invariants and failure modes

- stable facts are never encoded only in prose;
- serialization order is deterministic;
- fixes reference registered sources and half-open ranges;
- related diagnostics are non-recursive;
- debug evidence is absent from standard output;
- source text policy depends on source kind, not local path spelling;
- unknown or incomplete provenance remains explicit instead of fabricating an
  exact location.

Structural tests cover typed values, roles, source ranges, fixes, debug
visibility, clean stdout, success/failure envelopes, and the negative corpus.
G3 may now add occurrence-aware Box provenance without changing this schema.
