# `FRS-*` Diagnostic Codes — Engineering Reference

> **Looking for the user-facing list?** See §7 of
> [`docs/faust-error-model-en.md`](faust-error-model-en.md), which groups every
> code by what it means for a Faust programmer. This document is the maintainer
> view: where each code is raised, which ones are currently unreachable and why,
> how the set is derived from source, and what stops it drifting.

This is the engineering reference for the frozen list of stable diagnostic
codes (`FRS-*`) emitted by the compiler's structured diagnostics (`--error-format json`,
`--error-format human`, and the `--check` mode). It is part of the P0 phase of
`porting/mcp-server-analysis-and-plan-2026-07-21-en.md` (§1.4.5: "Stable codes
become a public contract"), and exists so that a consumer — CI, an IDE, or a
future MCP server — can treat the code set as a versioned API rather than
re-deriving it from source on every change.

**Freeze rule.** Adding a new code is fine. Renumbering or repurposing an
existing code is not — it silently breaks every consumer that matched on it.
`crates/compiler/src/cli/tests.rs::frozen_frs_code_table_matches_source`
enforces this by re-running the exact extraction command below and diffing
the result against the table in this document; both adding an undocumented
code and renumbering a documented one fail that test.

**Source of truth / how this table was generated.** The canonical way to
enumerate every code actually present in source is:

```bash
grep -rhoE 'FRS-[A-Z]+-[0-9]+' --include=*.rs crates/ | sort -u
```

This currently returns **33 codes** across **10 stage-family namespaces**:
`FRS-LEX-*` (1), `FRS-PARSE-*` (3), `FRS-SRC-*` (3), `FRS-EVAL-*` (8),
`FRS-PROP-*` (5), `FRS-COMP-*` (2), `FRS-UI-*` (1), `FRS-FIR-*` (2),
`FRS-SFIR-*` (8), `FRS-CODEGEN-*` (1).

Backend emitters additionally own a **separate, finer taxonomy** of 27 codes
shaped `FRS-CGEN-<LANG>-NNNN` (ASC, C, CLIF, CPP, INTERP, JULIA, RUST, WASM).
Those are *not* part of this table and do not appear in `diagnostics::codes`: they
travel inside `FRS-CODEGEN-0001` diagnostics as typed `detail_code` and
`codegen_code` fact values, the same way FIR verifier codes travel inside
`FRS-FIR-000{1,2}` as `detail_code` and `fir_code`.
Note they do not match the extraction regex either (the extra `-<LANG>` segment
means `FRS-[A-Z]+-[0-9]+` never matches), so they cannot silently leak into the
frozen set.

Note the family prefix (`LEX`, `PARSE`, ...) is a naming convention only; the
JSON payload's `"stage"` field comes from the independent `diagnostics::Stage`
enum and does not always equal the family name (e.g. every `FRS-SFIR-*` code
reports `"stage": "transform"`, not `"stage": "sfir"` — there is no `Sfir`
`Stage` variant). Both are listed per code below.

## Important caveat: a few codes are currently unreachable or unused

The extraction command above is a textual grep over `.rs` source, not a
reachability analysis. Building this table required tracing every code from
its `diagnostics::codes::*` constant to an actual call site, and that surfaced
real gaps, recorded here rather than papered over:

- **`FRS-SRC-0001`, `FRS-SRC-0002`, `FRS-SRC-0003`** are defined in
  `crates/diagnostics/src/codes.rs` and listed in `codes::all_codes()`, but no
  code anywhere in the workspace ever constructs a `Diagnostic` with them.

  **Wired up 2026-07-21.** These were never dead reservations:
  `parser::source_reader::SourceReaderError` has exactly three variants that map
  one-to-one onto the three codes, and all three fire in practice.
  `SourceReaderError::to_diagnostics` now builds a real bundle for each, and
  `CompilerError::import` attaches it, so source-loading failures no longer fall
  through to the `code: null` envelope:

  | Variant | Code | Diagnostic content |
  |---|---|---|
  | `Io { path, message }` | `FRS-SRC-0001` | path note + readability help |
  | `UnresolvedImport { .. }` | `FRS-SRC-0002` | span on the `import(...)` directive, import name, importing file, ordered list of searched directories, `-I` help |
  | `ImportCycle { path }` | `FRS-SRC-0003` | cycle note + help to break it |

  The reference C++ compiler reports the same conditions as bare strings
  (`ERROR : unable to open file <name>`), with no location and no searched
  paths, so this is deliberately more informative than parity rather than a
  port of it.

- **`FRS-COMP-0001`, `FRS-COMP-0002`, `FRS-COMP-0003` were retired**
  (2026-07-21) — see "Retired codes" below.
- **`FRS-LEX-0001`** is defined and its call site
  (`crates/parser/src/lib.rs:1926`) is live code, but it is not reachable
  from any DSP text found during this audit: `crates/parser/src/grammar/faustlexer.l`
  ends with a catch-all `. 'EXTRA'` rule, so every single byte the lexer
  sees matches *some* token (an `EXTRA` token in the worst case) and the
  failure surfaces one layer up as a `FRS-PARSE-0001` parse error instead of
  a `lrpar::LexParseError::LexError`. Genuinely invalid bytes (e.g. a
  non-UTF-8 byte sequence) are rejected even earlier, at file read time,
  before lexing starts, with no diagnostics bundle at all.

  **Decision (2026-07-21): kept deliberately.** Unlike the dormant `FRS-SRC-*`
  / `FRS-COMP-000{1,2,3}` declarations above, this one is not an unused
  constant: it is one arm of an exhaustive `match` over `lrpar::LexParseError`,
  a third-party enum (`parser_code_for_lex_parse_error`). Removing the code
  would not remove any code path — it would only force that arm to report a
  less accurate code. It becomes reachable again if the lexer's catch-all rule
  is ever narrowed.
- **`FRS-FIR-0001`** (verifier *error*, as opposed to `FRS-FIR-0002`
  warnings) requires the FIR verifier to reject FIR text that a
  *successful* front-end run produced — i.e. a compiler bug, not a user
  DSP mistake. No corpus file triggers it; only `--fir-fixture` bring-up
  fixtures could, and the eight built-in fixtures
  (`--list-fir-fixtures`) are all valid by construction.
- **`FRS-EVAL-0100` was removed from this table** (2026-07-21). It never came
  from `diagnostics::codes`: it was a literal string in
  `crates/diagnostics/src/lib.rs`'s
  own unit test `bundle_counts_error_severity_only`, picked up only because the
  extraction is textual. Documenting it made the table promise a public code
  that nothing emits. The test now uses a real registered code
  (`EVAL_GENERIC_FAILURE`), so the extraction no longer sees a phantom.

Nothing here blocks freezing: a dormant or unreachable code is still a valid,
stable reservation. But a consumer should not assume every documented code is
observable in practice today.

## Code table

### `FRS-LEX-*` — Lexer (1 code)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-LEX-0001` | `lexer` (via `Stage::Parser` in practice — see caveat) | Lexer encountered an invalid token sequence. | `crates/parser/src/lib.rs:1926` (`parser_code_for_lex_parse_error`); currently unreachable, see caveat above. |

### `FRS-PARSE-*` — Parser (3 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-PARSE-0001` | `parser` | Parser encountered an unexpected token. | `crates/parser/src/lib.rs:1917` (default case), `:1927` (`LexParseError::ParseError`) |
| `FRS-PARSE-0002` | `parser` | Parser recovered from an error and emitted recovery diagnostics (warning/remark severity). | `crates/parser/src/lib.rs:1913` |
| `FRS-PARSE-0003` | `parser` | Parser encountered an invalid literal form. | `crates/parser/src/lib.rs:1915` |

### `FRS-SRC-*` — Source reader (3 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-SRC-0001` | `source_reader` | Source reader I/O failure (unreadable file, directory passed as input). | `SourceReaderError::Io` → `to_diagnostics` (`crates/parser/src/source_reader.rs`) |
| `FRS-SRC-0002` | `source_reader` | Imported file could not be resolved. Carries a span on the `import(...)` directive and the ordered list of searched directories. | `SourceReaderError::UnresolvedImport` → `to_diagnostics` |
| `FRS-SRC-0003` | `source_reader` | Import graph contains a cycle. | `SourceReaderError::ImportCycle` → `to_diagnostics` |

### `FRS-EVAL-*` — Box evaluation (8 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-EVAL-0001` | `eval` | `process` definition is missing. | `crates/eval/src/error.rs:403` |
| `FRS-EVAL-0002` | `eval` | Symbol lookup failed during eval (undefined symbol). | `crates/eval/src/error.rs:433` |
| `FRS-EVAL-0003` | `eval` | Arity mismatch detected during eval (e.g. too many arguments). | `crates/eval/src/error.rs:471,488` |
| `FRS-EVAL-0004` | `eval` | Invalid iteration construct detected during eval. | `crates/eval/src/error.rs:658` |
| `FRS-EVAL-0005` | `eval` | Symbol redefined with a different value in the same lexical scope. | `crates/eval/src/error.rs:620` |
| `FRS-EVAL-0006` | `eval` | Slider/numentry init value is outside the `[min, max]` range. | `crates/eval/src/error.rs:692` |
| `FRS-EVAL-0099` | `eval` | Generic eval failure fallback code (covers eval-error variants without a dedicated code). | `crates/eval/src/error.rs` (multiple sites, e.g. `:508,517,530,539,554,584,592,603,646,669,704`) |

### `FRS-PROP-*` — Box-to-signal propagation (5 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-PROP-0001` | `propagate` | Unsupported box node encountered in propagate. | `crates/propagate/src/error.rs:227,436` |
| `FRS-PROP-0002` | `propagate` | Arity mismatch in propagate composition rules (`seq`/`split`/`merge`/UI wiring). | `crates/propagate/src/error.rs:235,247,268,301,398,406,414` |
| `FRS-PROP-0003` | `propagate` | Recursion/projection contract mismatch in propagate (`rec` arity/alias). | `crates/propagate/src/error.rs:339` |
| `FRS-PROP-0004` | `propagate` | Automatic differentiation (`fad`/`rad`) reached a clock-domain boundary it cannot cross. | `crates/propagate/src/error.rs:548` |
| `FRS-PROP-0099` | `propagate` | Generic propagate failure fallback code. | `crates/propagate/src/error.rs:372,380,390,422` |

### `FRS-COMP-*` — Top-level compiler pipeline (3 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-COMP-0004` | `type_inference` | Signal type validation failed. | `crates/compiler/src/error_mapping.rs` |
| `FRS-COMP-0005` | `compiler` | Parse reported no errors yet exposed no root node. Internal invariant guard — reaching it means a compiler bug, not a DSP mistake (an empty file fails later with `FRS-EVAL-0001`). | `CompilerError::missing_root` |
| `FRS-COMP-0006` | `transform` | `--table-init const` embedded an explicit `ma.SR` value in a generated table; emitted as a non-fatal `--warn` diagnostic. | `Compiler::pipeline_to_signals` |

### `FRS-CODEGEN-*` — Backend emission (1 code)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-CODEGEN-0001` | `codegen` | Backend code generation failed while emitting from FIR. Carries typed `backend`, `detail_code`, and `codegen_code` fields. | `CompilerError::codegen_diagnostics`, via all backend variants |

One code covers every backend deliberately: the failure class is identical and
the backend is a parameter, so the discriminating detail rides in typed fields rather
than multiplying near-identical `FRS-*` codes.

`FRS-COMP-0001`..`0003` are retired; the numbering gap is deliberate (see
below).

### `FRS-UI-*` — User-interface layout (1 code)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-UI-0001` | `propagate` | Two or more UI controls claim the same runtime address, so they are indistinguishable to every host. Carries typed `ui_path`, `control_count`, and `control_labels` fields, and one label per conflicting declaration. | `crates/compiler/src/ui_paths.rs` |

The C++ compiler raises the equivalent `path '...' is already used` while
serializing JSON. Rust checks the grouped `UiProgram` right after propagation
instead, so the same program is rejected regardless of the selected backend.

### `FRS-FIR-*` — FIR verifier (2 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-FIR-0001` | `fir` | FIR verifier error diagnostic (fatal; verifier code in `detail_code` and typed `fir_code`). | `crates/compiler/src/json_naming.rs:27`; currently unreachable from any known DSP input — see caveat. |
| `FRS-FIR-0002` | `fir` | FIR verifier warning diagnostic (verifier code in `detail_code` and typed `fir_code`); promoted to fatal under `--fir-verify-strict`. | `crates/compiler/src/json_naming.rs:28`; reachable for an ordinary DSP under `--fir-verify-strict`. |

### `FRS-SFIR-*` — Signal-to-FIR lowering (8 codes)

| Code | Stage | Meaning | Raised at |
|---|---|---|---|
| `FRS-SFIR-0001` | `transform` | Invalid options passed to signal→FIR lowering. | `crates/compiler/src/json_naming.rs:51` |
| `FRS-SFIR-0002` | `transform` | Empty signal list provided to signal→FIR lowering. | `crates/compiler/src/json_naming.rs:52` |
| `FRS-SFIR-0003` | `transform` | Signal outputs arity mismatch in signal→FIR lowering. | `crates/compiler/src/json_naming.rs:53` |
| `FRS-SFIR-0004` | `transform` | Unsupported signal node in signal→FIR lowering. | `crates/compiler/src/json_naming.rs:54`; reachable, e.g. `tests/corpus/err_fad_rad_temporal.dsp`. |
| `FRS-SFIR-0005` | `transform` | Unsupported binary operator in signal→FIR lowering. | `crates/compiler/src/json_naming.rs:55` |
| `FRS-SFIR-0006` | `transform` | Input index out of range in signal→FIR lowering. | `crates/compiler/src/json_naming.rs:56` |
| `FRS-SFIR-0007` | `transform` | Clocked node (`ondemand`/`upsampling`/`downsampling`) reached signal→FIR lowering before the clock-domain back half is ported. | `crates/compiler/src/json_naming.rs:57` |
| `FRS-SFIR-0008` | `transform` | Clock-environment inference / hierarchical-graph validation failed. | `crates/compiler/src/json_naming.rs:58` |
| `FRS-SFIR-0009` | `transform` | Foreign runtime variable `count` accessed under `-ec`/`-os` (no block count exists in `control`/`frame`). | `crates/compiler/src/json_naming.rs:59` |
| `FRS-SFIR-0010` | `transform` | Block-sensitive reverse-AD operation (`BlockReverseAD`/`ReverseTimeRec`) under `-os`: block-boundary semantics have no one-sample meaning (D2). | `crates/compiler/src/json_naming.rs:62` |

## The no-bundle fallback (`code: null`)

Some `CompilerError` variants carry no `DiagnosticBundle` at all — backend
codegen failures (`Codegen`, `CodegenC`, `CodegenJulia`, `CodegenInterp`,
`CodegenWasm`) and source/import failures (`Import`, `MissingRoot`). None of
the codes in this table apply to them. Under `--error-format json`,
`crates/compiler/src/cli/diagnostics.rs::format_fallback_diagnostics_json`
still emits a single-diagnostic envelope for these so stdout is always valid
JSON (D1), but with `"code": null` instead of a real `FRS-*` code — this is
intentional, not an omission from this table, and consumers should treat
`code == null` as "unstructured legacy error text" rather than look it up
here.

## Retired codes — never reassign

Deleting a code that was never emitted is safe: no consumer can have matched on
it. Reusing its *number* for a different meaning later is not — that is the same
silent break the freeze rule prevents, just delayed. Retired numbers are
therefore burned permanently.

| Code | Retired | Why |
|---|---|---|
| `FRS-COMP-0001` | 2026-07-21 | "parse stage failed" — already covered by `FRS-PARSE-*`, with spans the wrapper lacked |
| `FRS-COMP-0002` | 2026-07-21 | "eval stage failed" — already covered by `FRS-EVAL-*` (incl. the `0099` fallback) |
| `FRS-COMP-0003` | 2026-07-21 | "propagate stage failed" — already covered by `FRS-PROP-*` (incl. `0099`) |
| `FRS-EVAL-0100` | 2026-07-21 | never a code — a literal in a unit test, captured by the textual extraction |

`FRS-COMP-0004` is deliberately **not** renumbered into the gap left by
`0001`..`0003`: renumbering a live code is the one operation the freeze rule
forbids. A gap in the numbering is the correct end state.

## Where this is enforced

- `crates/compiler/src/cli/tests.rs::frozen_frs_code_table_matches_source` —
  re-runs the extraction grep and diffs it against the set documented above;
  fails on an undocumented new code or a renumbered existing one.
- `crates/compiler/src/cli/tests.rs::code_registry_matches_frozen_table` —
  checks that the runtime registry `diagnostics::codes::all_codes()` lists exactly
  the codes documented here, in both directions. Added 2026-07-21 after the two
  were found to have silently diverged (`FRS-EVAL-0006` was emitted but absent
  from the registry).
- `crates/diagnostics/src/codes.rs`'s own `all_codes_follow_stable_format` /
  `all_codes_are_unique` unit tests check the format/uniqueness invariants of
  the registered set.
- `cargo run -p xtask -- diagnostics-quality-check` — requires every declared
  code to be registered *and* to have a table row in **both** this document and
  the user-facing §7 of `docs/faust-error-model-en.md`, so the reader-friendly
  list cannot quietly fall behind this one. It also requires every variant of a
  serialized diagnostics enum (`Severity`, `Stage`, `DiagnosticCategory`,
  `LabelRole`, `TraceKind`, `Applicability`, `SourceKind`) to appear in
  `docs/diagnostics-v2.schema.json`, and rejects any new code that recovers
  machine meaning from note text.
- `crates/compiler/tests/cli_diagnostics_channel.rs` — validates the v2 payload
  of every negative-corpus entry against the published schema, and enforces the
  single-JSON-document-on-stdout contract.
- `crates/compiler/tests/machine_applicable_fixes.rs` — applies every
  `machine_applicable` fix for real and requires the diagnostic it targets to
  disappear without introducing a new one.
