# Compiler Diagnostics V2 — G1 Immutable Source Maps

> Date: 2026-07-28
>
> Source commit: `cc1a4a84`
>
> Plan:
> `compiler-diagnostics-v2-analysis-and-improvement-plan-2026-07-28-en.md`
>
> Phase: G1

## 1. Implemented boundary

The `diagnostics` crate now owns immutable source snapshots independently of
parser trees and diagnostic rendering:

- `SourceMap` is an immutable, cheaply cloned collection;
- `SourceMapBuilder` is the only mutable construction surface;
- `SourceId` is a compact map-local identifier;
- `SourceRange` is a canonical half-open UTF-8 byte range;
- `SourceFile` retains the exact source text, source kind, line index, logical
  path, and SHA-256 `ContentHash`;
- `HumanPosition` uses one-based display cells with four-column tab stops and
  Unicode display width;
- `LspPosition` uses zero-based UTF-16 code units.

The parser registers in-memory entry sources, file entry sources, imported
files, and embedded virtual-library sources. `DiagnosticBundle` retains the
resulting map. The human CLI renderer reads a registered snapshot before
falling back to the host filesystem, so a diagnostic cannot accidentally show
text written after the compilation that produced it.

## 2. Coordinate contract

The canonical contract is:

```text
SourceRange = (SourceId, start_byte .. end_byte)
```

Both bounds must be UTF-8 character boundaries, `start <= end`, and `end` may
equal the snapshot byte length. Source text is never newline-normalized. The
line index recognizes LF, CRLF, and bare CR while excluding line terminators
from snippets.

`SourceSpan` remains the v1 compatibility type. Its line and column values are
one-based Unicode-scalar positions; `end_col` is treated as the existing
half-open caret boundary. `SourceMap::to_source_span` and
`SourceMap::from_source_span` provide the explicit conversion boundary.

Human display columns and LSP columns are deliberately separate. A tab,
combining mark, wide CJK scalar, or astral scalar can have different UTF-8,
display-cell, Unicode-scalar, and UTF-16 widths.

## 3. API mapping and compatibility

| API | Mapping | Rationale and impact |
|---|---|---|
| `SourceMap`/`SourceFile` | adapted | C++ keeps mutable/global source state; Rust snapshots one compilation immutably |
| `SourceId`/`SourceRange` | adapted | canonical byte coordinates replace ambiguous global line/column state internally |
| `SourceSpan` | 1:1 compatibility surface | existing public diagnostics and v1 JSON retain file/line/column fields |
| human snippet lookup | adapted | snapshot-first lookup prevents stale filesystem reads; output shape is unchanged |
| v1 JSON | 1:1 | source maps are not serialized by the v1 renderer |
| parser `ParseOutput` | adapted | source snapshots travel inside its existing `DiagnosticBundle`; structural parse data is unchanged |

No C/C++ ABI, Faust syntax, IR node layout, diagnostic code, exit status, or
generated-code contract changes in G1.

## 4. Structural risks and mitigations

| Risk | Mitigation |
|---|---|
| byte bound splits UTF-8 | `validate_range` rejects non-character boundaries |
| source file changes after compile | renderer prefers immutable `SourceMap` text |
| CRLF is counted as two lines | line-index test treats CRLF as one terminator |
| terminal and LSP columns diverge | separate typed conversions with Unicode fixture |
| imported source omitted | parser integration tests assert entry/import registration order and kinds |
| source-map metadata changes v1 JSON | regression test compares v1 output with and without a map |
| duplicate source storage | builder deduplicates identical `(name, kind, text)` snapshots |

The source map is a session-side table rather than data embedded in
hash-consed `TreeId` nodes. This is intentional: source occurrences are not
semantic identity, as demonstrated by G0.

## 5. Test corpus

Focused tests cover:

- tabs;
- combining characters;
- CJK wide characters;
- an astral emoji requiring two UTF-16 code units;
- LF, CRLF, and bare CR;
- multi-line ranges;
- invalid UTF-8 boundaries;
- stable content hashes;
- file, memory, imported-file, and virtual-library registration;
- rendering from the compiled snapshot after the backing file changes;
- byte-for-byte invariance of v1 JSON with source-map metadata present.

## 6. G1 pass result

- snippets resolve for file, memory, imported, and virtual sources;
- renderer tests prove the exact compiled snapshot wins over later file text;
- Unicode, tabs, CRLF, multi-line, human, and LSP coordinates have structural
  tests;
- `SourceSpan` remains available through explicit compatibility conversions;
- existing v1 JSON serialization does not inspect or emit the source map.

G2 may define the opt-in typed/versioned schema on top of this source
foundation.
