# Compiler diagnostics v2 — G6 parser and import recovery

**Date:** 2026-07-28

**Baseline:** `3f42babc` (`Preserve diagnostics through FIR and backends`)

## Objective

G6 turns parser recovery and source-loading failures into actionable,
machine-readable diagnostics. It preserves the parser diagnostics of sources
loaded by `component(...)` and `library(...)`, and represents import cycles as
ordered edges instead of reporting only the repeated file.

## Parser recovery

The parser engine's repair sequences are converted once into typed recovery
data:

- `expected_tokens` contains normalized Faust lexemes rather than grammar
  terminal names such as `ENDDEF`;
- `unexpected_token` records the source token when one exists;
- an insertion is machine-applicable only when every minimum repair is the
  same singleton insertion;
- missing `;`, `)`, `]`, and `}` produce an exact zero-width `TextEdit`;
- a missing semicolon labels the previous non-whitespace token as a likely
  cause;
- a missing closing delimiter labels the unmatched opening delimiter;
- a keyword-like token within edit distance two of an expected keyword gets a
  `MaybeIncorrect` replacement, never a machine-applicable edit.

Several recovery reports at the same byte span collapse into one primary
diagnostic. The other reports remain available as `related` diagnostics, so
human output is quieter without discarding machine evidence.

The legacy rendered parser strings remain in `ParseOutput::errors` for
compatibility and error counts. They are no longer the source of structured
diagnostic fields.

## Nested source diagnostics

Evaluator source caching now stores `DiagnosticBundle`, not `Vec<String>`.
When parsing a loaded component or library fails, `EvalError` retains the
original bundle, including its stable parser codes, labels, source snapshots,
facts, and suggested fixes.

The compiler appends an evaluator context diagnostic to that bundle instead
of flattening parser errors into notes. If the nested bundle has a source map,
it remains authoritative; the entry source map is used only as a fallback.

## Import cycles

`ImportCycleEdge` records:

```text
importing file -> resolved imported file @ optional import site
```

Both textual and structural import expanders retain an ordered active path and
edge stack. On a repeated file they slice the stack at the first occurrence
and append the closing edge. `SourceReaderError::ImportCycle` therefore owns a
deterministic closed cycle.

The diagnostic exposes the path as the typed `import_cycle` string list and
adds one `import_site` label per located edge. The final edge is primary
because removing it is the most local way to break the detected recursion;
earlier edges are secondary context.

## API mapping

| Surface | Mapping | Compatibility impact |
| --- | --- | --- |
| parser grammar and accepted Faust | `1:1` | no grammar or recovery-policy change |
| `ParseOutput::errors` | `1:1` compatibility | rendered strings remain available |
| parser recovery diagnostics | Rust extension | typed facts, labels, related reports, and fixes |
| `SourceReaderError::ImportCycle` | `adapted` | adds an ordered `cycle` payload |
| `ImportCycleEdge` | Rust extension | public structured cycle edge |
| `EvalError::SourceParseFailure` | `adapted` | `DiagnosticBundle` replaces `Vec<String>` |
| evaluator loaded-source cache | internal adaptation | retains the original bundle |

The `SourceReaderError` and `EvalError` Rust enums are source-breaking for
exhaustive external pattern matches. Stable diagnostic codes, CLI exit status,
Faust semantics, generated code, and external C/C++ ABI surfaces are
unchanged.

## Failure modes and mitigation

| Risk | Mitigation |
| --- | --- |
| unsafe automatic typo rewrite | typo edits are always `MaybeIncorrect` |
| ambiguous parser recovery becomes an exact fix | exact edits require identical singleton repairs |
| grammar token names leak into tools | terminal-to-Faust lexeme normalization |
| cascade suppression hides evidence | same-span reports move to `related` |
| cycle order varies with hash iteration | ordered path/edge stacks are authoritative |
| nested parser codes become evaluator notes | original bundle is preserved and extended |
| stale filesystem snippets | nested immutable source snapshots remain attached |

## Structural coverage

- a missing final semicolon exposes `expected_tokens` and an exact insertion;
- a missing closer labels its opening delimiter and offers an exact insertion;
- a three-file cycle retains all three ordered edges and import sites;
- an invalid loaded component retains `FRS-PARSE-0001` and the child-file
  label through the compiler facade;
- imported-file parser diagnostics retain their original source origin.

## Phase gate

G6 passes when formatting, Rustdoc, Clippy, workspace tests, golden snapshots,
and structural checkers are green. G7 can add semantic suggestions and warning
policy without reopening parser or import transport.
