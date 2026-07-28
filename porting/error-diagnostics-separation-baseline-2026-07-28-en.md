# Error and Diagnostics Separation Baseline

**Date:** 2026-07-28

**Baseline commit:** `8a909843` (`main-dev`)

**Plan:** `error-diagnostics-separation-analysis-and-porting-plan-2026-07-28-en.md`

**Status:** E0 contract freeze and compatibility decision

## 1. Purpose

This document freezes the observable contracts that must survive the
`errors`-to-`diagnostics` migration. It also records the Rust package
compatibility decision required by phase E0.

The migration changes ownership names and internal adapters. It does not
authorize changes to Faust compilation behavior, diagnostic classification,
rendering, exit status, or foreign APIs.

## 2. Current package inventory

The package at `crates/errors`:

- is named `errors`;
- has workspace version `0.5.0`;
- does not declare `publish = false`;
- contains `src/lib.rs` and `src/codes.rs`;
- has four direct workspace consumers.

| Consumer | Dependency use |
|---|---|
| `parser` | canonical diagnostics, codes, source spans, and parser-output bundle |
| `eval` | codes, diagnostic construction, and `IntoDiagnostic` |
| `propagate` | codes, diagnostic construction, and `IntoDiagnostic` |
| `compiler` | facade bundles, enrichment, render input, and stable-code lookup |

No compiler-core, backend, runtime, FFI, or tooling package outside those four
depends directly on `errors`.

Repository history contains release tags, but the repository documents no
`cargo publish` workflow, crates.io package ownership, or supported external
consumer of the package named `errors`. All source imports and manifest
dependencies found at this baseline are workspace-local.

## 3. Rust compatibility decision

Phase E1 will perform a direct rename:

```text
crates/errors    -> crates/diagnostics
errors           -> diagnostics
```

No deprecated `errors` compatibility package will be retained.

Rationale:

- the approved plan recommends a direct rename on the current pre-1.0 line;
- every repository-visible consumer is migrated atomically in one workspace;
- a compatibility package would preserve the misleading name and create two
  apparent owners for the stable code registry;
- `compiler` will re-export the diagnostic types appearing in its public
  facade, avoiding a new direct dependency for ordinary compiler clients.

Compatibility impact:

- Faust CLI: unchanged;
- C/C++ API: unchanged;
- WebAssembly ABI: unchanged;
- direct Rust imports from package `errors`: breaking and release-noted;
- diagnostic types and values: 1:1 path rename;
- unused `errors::CRATE_NAME` and `errors::crate_id()`: removed.

If an external direct consumer is discovered after this decision, it must
migrate its dependency/import path. Reintroducing a shim requires an explicit
compatibility decision and removal release; it is not an implicit fallback.

## 4. Frozen diagnostic registry

`errors::codes::all_codes()` contains 34 active codes at the baseline:

```text
FRS-CODEGEN-0001
FRS-COMP-0004
FRS-COMP-0005
FRS-EVAL-0001
FRS-EVAL-0002
FRS-EVAL-0003
FRS-EVAL-0004
FRS-EVAL-0005
FRS-EVAL-0006
FRS-EVAL-0099
FRS-FIR-0001
FRS-FIR-0002
FRS-LEX-0001
FRS-PARSE-0001
FRS-PARSE-0002
FRS-PARSE-0003
FRS-PROP-0001
FRS-PROP-0002
FRS-PROP-0003
FRS-PROP-0004
FRS-PROP-0099
FRS-SFIR-0001
FRS-SFIR-0002
FRS-SFIR-0003
FRS-SFIR-0004
FRS-SFIR-0005
FRS-SFIR-0006
FRS-SFIR-0007
FRS-SFIR-0008
FRS-SFIR-0009
FRS-SFIR-0010
FRS-SRC-0001
FRS-SRC-0002
FRS-SRC-0003
```

The retired-code reservations in `docs/diagnostics-codes-en.md` remain
authoritative and must not be reused. E1-E5 may rename the registry path but
must not add, remove, or renumber a code unless a separate diagnostic-contract
change is approved.

Enforcement already exists in:

- `errors::codes::tests::all_codes_follow_stable_format`;
- `errors::codes::tests::all_codes_are_unique`;
- `compiler::cli::tests::frozen_frs_code_table_matches_source`;
- `compiler::cli::tests::code_registry_matches_frozen_table`.

## 5. Frozen public data contracts

The following diagnostic data remains 1:1 through the rename:

| Type | Frozen contract |
|---|---|
| `Severity` | ordered semantic variants `Error`, `Warning`, `Remark` |
| `Stage` | source-reader through compiler stage attribution |
| `DiagnosticCode` | stable static string identifier |
| `SourceSpan` | repository-portable path plus 1-based inclusive coordinates |
| `Label` | primary/secondary span plus message |
| `Diagnostic` | severity, stage, code, message, labels, notes, help |
| `DiagnosticBundle` | deterministic ordered sequence and error count |

Neither `Diagnostic` nor `DiagnosticBundle` is a causal error-chain node.
They deliberately do not implement `std::error::Error`.

Public cross-package occurrences at the baseline include:

- `parser::ParseOutput::diagnostics: DiagnosticBundle`;
- `parser::SourceReaderError::to_diagnostics() -> DiagnosticBundle`;
- every `compiler::CompilerError` variant carrying a bundle;
- `CompilerError::diagnostics() -> Option<&DiagnosticBundle>`.

E1 will re-export the canonical diagnostic types from `compiler`. E3 may add a
non-optional bundle accessor while retaining the existing accessor according
to the approved plan.

## 6. Frozen rendering and process contracts

Representative baseline gates:

| Family | Fixture/gate | Required outcome |
|---|---|---|
| success | `rep_01_passthrough.dsp --check --error-format json` | exit 0; one clean JSON document; empty diagnostics array |
| source | missing import integration case | exit 1; `FRS-SRC-*`; no non-JSON stdout bytes |
| parse | `err_01_parse_missing_rhs.dsp` | exit 1; `FRS-PARSE-*` |
| eval | `err_02_eval_missing_process.dsp` | exit 1; `FRS-EVAL-*` |
| propagate | `err_03_propagate_split_mismatch.dsp` | exit 1; `FRS-PROP-*` |
| type | `rep_74_soundfile_basic.dsp` | exit 1; `FRS-COMP-0004` |
| signal-to-FIR | `err_fad_rad_temporal.dsp` | exit 1; `FRS-SFIR-*` |
| FIR | passthrough plus `--fir-verify-strict` | exit 1; `FRS-FIR-*` |
| codegen | backend emission failure integration case | exit 1; `FRS-CODEGEN-0001` plus backend notes |

The executable channel contract is:

- normal human diagnostics go to stderr;
- `--check --error-format json` emits exactly one JSON document on stdout;
- successful JSON check output uses the same schema with an empty diagnostics
  array;
- failures use process status 1;
- CLI usage errors remain Clap status 2 and are outside the compiler diagnostic
  schema.

The exact human and JSON shapes are frozen by:

- `crates/compiler/src/cli/tests.rs` renderer snapshots;
- `crates/compiler/tests/cli_diagnostics_channel.rs`;
- `crates/compiler/tests/diagnostic_errors.rs`;
- the Rust golden corpus under `tests/golden/rust`.

## 7. Compiler error-source baseline

`CompilerError` implements `std::error::Error` with the default `source()`
implementation, so every variant currently returns `None`.

E3 must change source traversal according to this table:

| Variant | Expected source after E3 |
|---|---|
| `Import` | `SourceReaderError` |
| `Eval` | `EvalError` |
| `Propagate` | `PropagateError` |
| `Type` | signal-inference error |
| `ExecutionOptions` | `ExecutionOptionsError` |
| `Transform` | `SignalFirError` |
| `CodegenCpp` | C++ backend error |
| `CodegenC` | C backend error |
| `CodegenJulia` | Julia backend error |
| `CodegenAsc` | AssemblyScript backend error |
| `CodegenCodebox` | Codebox backend error |
| `CodegenRust` | Rust backend error |
| `CodegenInterp` | Interpreter backend error |
| `CodegenCranelift` | Cranelift backend error on supported targets |
| `CodegenWasm` | WebAssembly backend error |
| `Parse` | none: aggregate parser/recovery outcome |
| `FirVerify` | none: aggregate verifier report |
| `MissingRoot` | none: compiler invariant report |

The source-chain change is additive Rust behavior. It must not affect
`Display`, diagnostic rendering, or exit policy.

## 8. Parser representation baseline

The parser currently exposes:

- `DiagnosticSeverity`;
- `ParserDiagnostic`;
- `ParserDiagnostic::code: Option<DiagnosticCode>`;
- `parser_code_for_message`, which selects a fallback code from severity and
  message substrings;
- `parser_ctx_to_bundle`, which translates the local representation.

E4 may change this internal/public Rust representation, but it must preserve:

- diagnostic ordering;
- error and recovery counts;
- source coordinates;
- error/warning/remark severity;
- emitted stable codes;
- human and JSON output;
- parser acceptance and recovery behavior.

Message text must cease to determine the stable code. Each parser emission
site must provide its category by construction.

## 9. Phase gates

Each implementation commit must pass:

```text
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p xtask -- golden-check
```

Focused gates from the plan remain mandatory for the owning phase. Any golden
refresh, code addition/removal, wording change, or exit-status change is
outside this baseline and requires a separately documented decision.
