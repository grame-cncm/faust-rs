# Compiler diagnostics v2 — G3 occurrence-aware Box provenance

Date: 2026-07-28

Plan:
`porting/compiler-diagnostics-v2-analysis-and-improvement-plan-2026-07-28-en.md`

## 1. Implemented representation

G3 implements the G0 hybrid without changing `TreeArena` equality:

- `BoxOriginId` identifies one parser-session source occurrence;
- `BoxOrigin` stores the semantic `TreeId`, exact `SourceLocation`, and
  definition/use role;
- `BoxProvenance` maps each hash-consed node to every origin in parse order;
- `LocatedBox` pairs one semantic node with one selected origin and verifies
  that the pair is coherent.

The legacy definition/use property store remains available, but it is no
longer the source of truth for ambiguity-sensitive diagnostic selection. It
continues to be last-write-wins for C++-mapping compatibility.

## 2. Parser and import boundaries

Every definition/use grammar action records an occurrence before updating the
compatibility property. Definition actions recover the identifier-token
location observed before the right-hand side was parsed, fixing the previous
behavior where the cursor could point at the final RHS token.

Structural imports parse in independent arenas. When an imported definition is
re-interned in the destination arena, G3 reconstructs the ordered
source-to-destination node map and copies every imported occurrence through
that map. Several source nodes may map to one destination node; their origins
remain distinct.

Public API mapping is `adapted`: C++ uses one global tree pool and one mutable
line property, while Rust preserves all occurrences in parser-session state.
There is no C ABI, generated-code, or semantic IR-layout change.

## 3. Diagnostic selection

For an error node present in several top-level definitions, compiler
diagnostics no longer choose the first structurally matching definition.
They build the deterministic definition-reference graph and select the first
owning definition reachable by breadth-first traversal from the configured
entry point.

Within that definition, definition-token locations delimit its lexical region.
Exactly one matching node occurrence becomes the primary `use_site` or
`operator` label. The enclosing definition and entry-point call remain
secondary labels. If zero or several candidates remain, the implementation
falls back to the conservative definition-level label and does not invent an
exact source position.

This handles nested `with`, `letrec`, and iteration syntax inside a top-level
owner because grammar occurrences retain their original lexical positions.
Generated nodes without a direct grammar occurrence keep the documented
fallback.

## 4. Structural non-regression tests

Tests prove that:

- two identical identifier uses share one `TreeId` but retain two ordered
  origins and two resolvable `LocatedBox` values;
- the real parser records both lines, not only a synthetic data structure;
- imported occurrences survive cross-arena cloning;
- `unused = missing; active = missing; process = active;` points to the
  reachable line 2 occurrence rather than the structurally identical line 1
  occurrence;
- definition, use, and call labels keep explicit machine roles.

## 5. Performance observation

The release G0 probe was rerun with 250,000 occurrences and 4,096 semantic
nodes:

| Representation | Build | Query | Estimated bytes |
|---|---:|---:|---:|
| dense origin sets | 1,704,625 ns | 271,125 ns | 1,146,904 |
| located occurrences | 370,167 ns | 41,541 ns | 2,000,000 |

The warmed release parser corpus test completed its test body in approximately
0.41 seconds on the same arm64 macOS host. These timings are observational,
not CI thresholds. The production representation allocates origins only for
grammar actions already carrying source properties and keeps located handles
at ambiguity-sensitive boundaries, matching the G0 cost model.

## 6. Invariants and failure modes

- occurrence identity never participates in semantic hash-consing;
- origin ids are deterministic within one parse session;
- candidate order is source/action order;
- imported provenance is copied only for mapped cloned nodes;
- reachability selection is bounded and deterministic;
- ambiguity produces a fallback, never arbitrary exact blame;
- canonical JSON ranges are still derived through the immutable G1
  `SourceMap`.

G4 may carry this source identity into Signal construction and typing.
