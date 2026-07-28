# Error and Diagnostics Separation Analysis and Porting Plan

**Date:** 2026-07-28

**Analyzed source:** `98496b4a` (`main-dev`)

**Scope:** the `errors` crate, phase-local Rust error types, parser
diagnostics, and the `CompilerError` facade

**Status:** analysis and proposed migration; no implementation in this
document

## 1. Review concern

A code review raised the following concern:

> There are some obvious C++ translations here such as the "errors" crate not
> containing idiomatic Rust errors.

The name-based observation is correct, but the implied diagnosis needs to be
split into two questions:

1. Does `crates/errors` contain non-idiomatic Rust error types?
2. Does the workspace make the distinction between errors and diagnostics
   sufficiently clear?

For the first question, `crates/errors` does not primarily contain error types
at all. It owns a structured diagnostic data model: severity, pipeline stage,
stable code, labels, notes, help, and aggregation. A diagnostic may be a
warning or remark, and a bundle may contain several messages. Making
`Diagnostic` or `DiagnosticBundle` implement `std::error::Error` would blur
their role rather than make the design more idiomatic.

For the second question, the criticism is justified. A crate called `errors`
that exports almost exclusively diagnostic presentation data creates a false
architectural promise. It also obscures the fact that operational errors are
already owned by the passes that can produce them.

The recommended correction is therefore:

- rename `errors` to `diagnostics`;
- keep fallible control flow in phase-local `Result<_, PhaseError>` types;
- keep diagnostics as structured, renderable reports;
- improve the conversion and causal-chain boundaries between both models;
- remove remaining string-based and duplicated diagnostic classification
  where it affects public compiler failures.

## 2. Current architecture

### 2.1 The `errors` crate is a diagnostics crate

`crates/errors` contains two source modules and exposes:

| API | Actual role |
|---|---|
| `Diagnostic` | One structured report with severity, stage, code, message, labels, notes, and help |
| `DiagnosticBundle` | Ordered aggregation of reports for one outcome |
| `DiagnosticCode` and `codes::*` | Stable machine-readable diagnostic taxonomy |
| `Severity` | Error, warning, or remark classification |
| `Stage` | Pipeline attribution |
| `SourceSpan`, `Label`, `LabelStyle` | Source presentation metadata |
| `IntoDiagnostic` | Adapter from a phase-local failure into one report |
| `CRATE_NAME`, `crate_id()` | Scaffold-era crate identity with no current consumer |

The registry currently contains 34 diagnostic codes. The crate intentionally
owns no renderer and no compiler-pass implementation. Its four direct
consumers are `parser`, `eval`, `propagate`, and `compiler`.

This is a sound low-level dependency direction. The misleading part is the
package and import name:

```text
errors::Diagnostic
errors::DiagnosticBundle
errors::Severity
```

Those paths imply throwable/fallible values while naming report data.

### 2.2 Operational errors are already phase-local

The main compiler passes and backends generally use ordinary Rust error
surfaces:

| Owner | Representative type | Rust error contract |
|---|---|---|
| parser/source loading | `SourceReaderError` | `Display` + `std::error::Error` |
| evaluator | `EvalError` | typed enum, `Display` + `std::error::Error` |
| propagation | `PropagateError` | typed enum, `Display` + `std::error::Error` |
| normalization | `NormalFormError` | typed enum, `Display` + `std::error::Error` |
| signal-to-FIR transform | `SignalFirError` | typed struct/code, `Display` + `std::error::Error` |
| FIR inlining | `FirInline*Error` | typed enums, `Display` + `std::error::Error` |
| code generators | backend-specific error types | `Display` + `std::error::Error` |
| compiler facade | `CompilerError` | typed enum, `Display` + `std::error::Error` |

This ownership is idiomatic: the crate that defines the operation defines the
failure type and returns it through `Result`. It also avoids a monolithic
cross-workspace error enum and keeps lower crates independent from the
compiler facade.

The architecture is consequently already closer to:

```text
phase operation
    -> Result<T, PhaseError>
    -> compiler context/enrichment
    -> Diagnostic or DiagnosticBundle
    -> CLI human/JSON renderer
```

than to the C++ global string/exception model.

### 2.3 The compiler facade carries both meanings

`CompilerError` represents unsuccessful control flow and therefore correctly
implements `std::error::Error`. Its variants also carry a
`DiagnosticBundle`, allowing the CLI and API clients to render stable,
source-aware output.

That dual payload is defensible because diagnostic enrichment sometimes needs
the arena, parser context, owning definition, source map, or backend code in
addition to the phase error itself. It should not be replaced by a
string-only error or by an untyped boxed error.

There are nevertheless concrete gaps:

- `impl std::error::Error for CompilerError {}` does not implement
  `source()`, so wrapped `SourceReaderError`, `EvalError`, `PropagateError`,
  execution-option errors, transform errors, and backend errors disappear
  from standard Rust causal traversal;
- `CompilerError::Type` stores a `Box<str>` even though signal typing already
  produces a typed `sigtype::rules::TypeError`;
- `CompilerError::diagnostics()` returns `Option<&DiagnosticBundle>` although
  every variant now carries a bundle;
- public enum variants can be constructed without the helper constructors,
  so comments, rather than the type system, currently protect the invariant
  that the phase error and diagnostic bundle describe the same failure.

These are more meaningful idiomatic-Rust improvements than adding an error
derive to the diagnostics model.

### 2.4 Conversion currently requires avoidable cloning

`IntoDiagnostic` consumes `self`, which follows Rust's `Into*` naming
convention. However, the compiler must retain the original phase error in
`CompilerError` and enrich the generated diagnostic. It therefore currently
does this for evaluation and propagation:

```rust
let mut diagnostic = error.clone().into_diagnostic();
```

The clone can include paths, visible-scope lists, source errors, or other
context. A borrowing conversion is a better fit for this call site:

```rust
pub trait ToDiagnostic {
    fn to_diagnostic(&self) -> Diagnostic;
}
```

This is an API adaptation, not a semantic change. Constructing a diagnostic
still allocates its owned display payload, but it no longer clones the complete
phase error merely to preserve it for the causal chain.

### 2.5 Parser diagnostics still have a parallel model

The parser context defines its own:

- `DiagnosticSeverity`;
- `ParserDiagnostic`;
- optional diagnostic code;
- `SourceLocation`.

`parser_ctx_to_bundle` later converts this model into the canonical diagnostic
types. When a parser emission did not provide a code,
`parser_code_for_message` classifies it by inspecting message text.

This is a genuine translation artifact and a maintenance risk:

- severity has two representations;
- a code can be absent internally even though the external code taxonomy is a
  stable contract;
- wording changes can silently change machine classification;
- the conversion boundary is farther from the point that knows the error
  category.

The parser needs some stage-local state for recovery, but it does not need a
second severity vocabulary or message-based code inference.

### 2.6 String errors still exist, but scope matters

The workspace contains some `Result<_, String>` helpers. Not every occurrence
should become a new public error enum. Private formatting helpers, test
utilities, and adapters whose typed error is immediately restored by the
caller do not justify a workspace-wide hierarchy.

The high-value cases are public or cross-layer failures where a string erases:

- the concrete source error;
- a stable category;
- structured fields needed for diagnostics;
- the `Error::source()` chain.

This plan targets those boundaries first and explicitly avoids a mechanical
conversion of every `String` into a one-variant error type.

## 3. Assessment of the criticism

The criticism is:

- **correct about naming and discoverability**: `errors` is the wrong name for
  a diagnostics schema;
- **incorrect if read as requiring diagnostics to implement
  `std::error::Error`**: warnings, remarks, labels, and bundles are report data,
  not error-chain nodes;
- **partly correct about the wider error surface**: the pass-local types are
  mostly idiomatic, but the compiler facade drops causal sources, one typed
  type-checking error is flattened to text, conversion clones are avoidable,
  and the parser maintains a duplicate diagnostic representation.

The desired architecture is not one central crate containing every Rust error.
It is a clear separation:

| Concern | Owner |
|---|---|
| fallible operation and recovery decisions | the phase/backend crate |
| typed failure value and causal source | the phase/backend crate |
| stable report vocabulary | `diagnostics` |
| source/context enrichment | the compiler facade or owning phase |
| human/JSON rendering and exit policy | the CLI/API boundary |

## 4. Proposed target architecture

### 4.1 Rename the crate

Rename:

```text
crates/errors        -> crates/diagnostics
package "errors"     -> package "diagnostics"
Rust path errors::*  -> diagnostics::*
```

The new crate header should state explicitly:

- it contains report data, not the universe of compiler errors;
- `Diagnostic` and `DiagnosticBundle` deliberately do not implement
  `std::error::Error`;
- phase errors remain local;
- diagnostic codes and output schema are compatibility contracts.

Remove `CRATE_NAME` and `crate_id()`. They are unused scaffold APIs and make a
pure data-model crate look like a registry.

The compiler's private `diagnostics.rs` module should be renamed to
`diagnostic_enrichment.rs` to prevent a name collision and make its role
explicit. The compiler facade should re-export the canonical diagnostic types
that appear in its public API, so normal clients do not need to discover a
transitive workspace package just to inspect `CompilerError`.

### 4.2 Preserve local typed errors

Do not move `EvalError`, `PropagateError`, `SignalFirError`, backend errors, or
runtime errors into `diagnostics`.

Each owning crate remains responsible for:

- the error enum/struct;
- its structured fields;
- `Display`;
- `std::error::Error`;
- `source()` when it wraps another error;
- conversions from lower-level errors where ownership is unambiguous.

This keeps `?` conversion and causal traversal local and prevents circular
dependencies.

### 4.3 Use a borrowing report conversion

Replace the consuming `IntoDiagnostic` contract with:

```rust
pub trait ToDiagnostic {
    fn to_diagnostic(&self) -> Diagnostic;
}
```

Implement it initially for `EvalError` and `PropagateError`, with exact
diagnostic-code/message/note/help parity. The compiler enriches the returned
value and stores the original error without cloning it.

Do not force all phases into this one-report trait:

- parser recovery may emit several diagnostics;
- source loading can naturally return a bundle;
- FIR verification already produces a report containing errors and warnings.

Those APIs should use explicit, accurately named borrowing methods such as
`to_diagnostics(&self)` when their cardinality is plural.

### 4.4 Restore causal error chains

Implement `CompilerError::source()` for variants with a concrete underlying
error:

- import;
- evaluation;
- propagation;
- execution-option validation;
- signal-to-FIR transform;
- each backend code-generation variant.

For parse recovery, FIR verification, and missing-root invariant failures,
return `None`: these variants represent an aggregate report or a facade
invariant rather than one nested `Error`.

Preserve signal typing's concrete error type in `CompilerError::Type` and
return it as the source. Before implementation, consolidate or explicitly
distinguish the two current `sigtype` types both named `TypeError`
(`ops::TypeError` and `rules::TypeError`) so the facade exposes the inference
error intentionally.

Add a non-optional accessor:

```rust
pub fn diagnostic_bundle(&self) -> &DiagnosticBundle
```

Keep `diagnostics() -> Option<_>` as a deprecated compatibility wrapper for
one release if external Rust API compatibility is required.

### 4.5 Converge the parser on the canonical vocabulary

Change parser emission sites so they assign a stable `DiagnosticCode` when the
condition is known. Then:

- use `diagnostics::Severity` directly;
- eliminate `DiagnosticSeverity`;
- remove `parser_code_for_message`;
- make a missing code an internal invariant failure, not a wording-based
  fallback;
- retain only the stage-local recovery fields that cannot yet be expressed by
  the canonical `Diagnostic`.

The final representation may be canonical `Diagnostic` values in `ParserCtx`
or a smaller private pending-diagnostic type. The choice must preserve parser
recovery counts, ordering, precise source expansion, and C++-parity wording.
It must not expose another public diagnostics schema.

### 4.6 Make consistency enforceable

After the mechanical migration, add a small architecture check that rejects:

- a workspace package or dependency named `errors`;
- new `errors::*` imports;
- `parser_code_for_message` or equivalent message-text classification;
- reintroduction of `CRATE_NAME`/`crate_id()` in `diagnostics`.

The check should not reject every `Result<_, String>` mechanically. Such a
rule would produce false positives and encourage meaningless wrapper types.
Typed-error audits should remain focused on public and cross-layer APIs.

## 5. Compatibility and parity

### 5.1 C++ parity

The C++ source remains provenance for error conditions and user-visible
wording, not for Rust ownership. The mapping is `adapted`:

- C++ exception/global-message events map to local Rust error values;
- Rust errors map to structured diagnostics;
- diagnostics are rendered at the boundary.

No phase in this plan may change whether a Faust program succeeds or fails.
Diagnostic codes, ordering, source labels, human text, JSON fields, and CLI
exit status remain parity gates.

### 5.2 Rust API compatibility

Renaming a package and public type path is a Rust API change even though it
does not affect the Faust CLI or C ABI. The current direct consumers are all
workspace packages, but the packages do not declare `publish = false`.

The recommended default for the current `0.5.0` development line is a direct
rename plus a release note and compiler-level re-exports. Before
implementation, confirm whether an independently published downstream crate
depends directly on package `errors`.

If such a consumer exists and must be supported, use a strictly time-boxed
compatibility package:

```text
errors (deprecated, re-exports diagnostics) -> diagnostics
```

Remove that shim in the next announced breaking release. Do not keep two
permanent owners for the code registry.

### 5.3 External surfaces

The following remain unchanged:

- Faust CLI accepted inputs and exit codes;
- human and JSON diagnostic schemas;
- stable `FRS-*` codes, including retired-code reservations;
- C/C++ and WebAssembly ABI layouts;
- backend/runtime error semantics.

## 6. Phased implementation plan

### E0 — Freeze contracts and resolve the Rust-package compatibility gate

Deliverables:

- inventory all direct `errors` consumers and public signatures containing its
  types;
- record the 34-code registry and retired-code table;
- freeze representative human and JSON diagnostic transcripts across source
  loading, parsing, eval, propagation, typing, FIR verification, and codegen;
- determine whether package `errors` has supported external Rust consumers;
- document the direct-rename or one-release-shim decision.

Pass criteria:

- no implementation starts before the package-compatibility decision is
  recorded;
- every stable diagnostic code and external rendering fixture has a baseline;
- the expected source-chain behavior for every `CompilerError` variant is
  tabulated.

Commit:

```text
Freeze error and diagnostics separation contracts
```

### E1 — Rename `errors` to `diagnostics`

Deliverables:

- rename directory, package, lockfile entry, dependency declarations, and Rust
  imports;
- update the root workspace inventory and current architecture documentation;
- rename compiler's private enrichment module;
- remove `CRATE_NAME` and `crate_id()`;
- re-export public diagnostic types from the compiler facade;
- regenerate code-graph/public-API reports rather than editing generated paths
  selectively.

Pass criteria:

- `cargo metadata` contains `diagnostics` and no unapproved `errors` package;
- only a previously approved compatibility shim may contain an `errors`
  package/import path;
- no diagnostic code or rendered output changes;
- all four current consumer crates build and test.

API mapping:

- diagnostic data types: `1:1`, with package/path rename only;
- `crate_id()`: removed scaffold API;
- Rust import path: `adapted` and release-noted;
- CLI/C ABI: unchanged.

Commit:

```text
Rename errors crate to diagnostics
```

### E2 — Make diagnostic conversion borrowing

Deliverables:

- introduce `ToDiagnostic::to_diagnostic(&self)`;
- migrate `EvalError` and `PropagateError`;
- remove the full-error clones used only to generate diagnostics;
- keep plural source-reader/parser/FIR conversions explicit;
- add structural tests proving conversion does not consume the phase error and
  preserves exact code/message/note/help payloads.

Pass criteria:

- evaluation and propagation diagnostics match the E0 snapshots;
- the compiler retains the original typed error after conversion without
  cloning it;
- no generic boxed/string error replaces phase-specific values.

API mapping:

- `IntoDiagnostic`: `adapted` to a borrowing conversion;
- emitted diagnostic contract: `1:1`.

Commit:

```text
Borrow phase errors when building diagnostics
```

### E3 — Restore compiler error sources and typed type failures

Deliverables:

- implement exhaustive `CompilerError::source()`;
- preserve the signal-inference `TypeError` rather than a `Box<str>`;
- resolve the duplicate `sigtype::TypeError` naming;
- add `diagnostic_bundle()`;
- retain/deprecate `diagnostics()` according to the E0 compatibility decision;
- test every facade variant's source/no-source classification.

Pass criteria:

- standard `Error::source()` traversal reaches every concrete phase/backend
  cause;
- aggregate-only variants deliberately return `None`;
- diagnostics remain byte-for-byte/schema-equivalent to the baseline;
- callers can always obtain the bundle without an `Option`.

API mapping:

- causal chain: `adapted`, additive behavior;
- type error payload: `adapted` from text to typed source;
- diagnostics accessor: additive, with compatibility policy documented.

Commit:

```text
Expose typed compiler error sources
```

### E4 — Remove the parser's parallel diagnostic taxonomy

Deliverables:

- assign codes at parser emission sites;
- use canonical severity values;
- remove message-text code inference;
- make pending parser diagnostics private or canonical;
- preserve recovery counters, source spans, ordering, and renderings;
- add parser tests that change wording while keeping code classification
  stable.

Pass criteria:

- `DiagnosticSeverity` and `parser_code_for_message` are gone;
- every parser error/warning/remark carries a code by construction;
- parser differential, transcript, and golden gates are unchanged;
- no public second diagnostics schema remains in `parser`.

API mapping:

- parser diagnostic internals: `adapted`;
- parser public diagnostic output: `1:1`;
- C++ parsing/recovery behavior: unchanged.

Commit:

```text
Unify parser diagnostics at emission sites
```

### E5 — Add architecture guardrails and finish documentation

Deliverables:

- add the narrow naming/message-classification architecture check;
- update diagnostics-code documentation and the porting architecture maps;
- document when to define a local error, when to emit a diagnostic, and when
  to add a causal source;
- record any intentionally retained string error with owner and rationale.

Pass criteria:

- CI rejects unclassified reintroduction of the old crate name and parser
  message classification;
- current documentation consistently says `diagnostics`;
- generated API/code-graph artifacts are current;
- all mandatory quality and parity gates pass.

Commit:

```text
Enforce error and diagnostics ownership
```

## 7. Validation matrix

Every implementation phase must run:

```text
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p xtask -- golden-check
```

Additional focused gates:

| Change | Required validation |
|---|---|
| package rename | `cargo metadata --no-deps`, all four consumer tests, documentation-link scan |
| conversion trait | eval/propagate diagnostic payload tests |
| causal sources | exhaustive `CompilerError` source-chain unit test |
| parser convergence | parser recovery tests, CLI diagnostic transcript check, human/JSON snapshots |
| stable registry | uniqueness/format tests and frozen `FRS-*` table check |
| generated documentation | code-graph regeneration and `structure-check` |

## 8. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Rust clients import `errors` directly | E0 publication audit; compiler re-exports; time-boxed shim only if required |
| rename accidentally changes stable codes or JSON | freeze registry/transcripts first; compare exact output |
| forcing all failures through one trait loses aggregate semantics | use singular borrowing trait only where cardinality is one |
| parser recovery changes while replacing its local model | migrate emission sites incrementally with recovery and differential tests |
| `CompilerError::source()` exposes the wrong layer | exhaustive per-variant source classification |
| typed-error cleanup expands without bound | restrict to public/cross-layer loss of structure; leave private strings alone unless harmful |
| diagnostic bundle and phase error drift | prefer constructors/conversion helpers and structural tests; evaluate a private wrapper only as a later breaking API change |

## 9. Non-goals

- Do not create one workspace-wide enum containing every compiler and runtime
  error.
- Do not make warnings, remarks, `Diagnostic`, or `DiagnosticBundle`
  implement `std::error::Error`.
- Do not add `thiserror` merely to shorten existing correct manual
  implementations.
- Do not convert every private `Result<_, String>` mechanically.
- Do not change Faust acceptance, backend behavior, CLI status, diagnostic
  wording, JSON schema, or stable code values as part of the rename.
- Do not redesign the public `CompilerError` representation in the same commit
  as the package rename.

## 10. Recommended outcome

The review should result in a clearer architecture, not a central error
warehouse:

```text
phase-local typed Error
        |
        | borrowing conversion + compiler context
        v
diagnostics::{Diagnostic, DiagnosticBundle}
        |
        v
CLI/API renderer
```

Renaming `errors` to `diagnostics` corrects the misleading boundary.
Borrowing conversion, causal `source()` chains, typed type failures, and parser
taxonomy convergence address the concrete idiomatic-Rust gaps without
sacrificing the structured diagnostics work already present.
