# graph

Shared graph algorithms crate — scaffold placeholder.

## Intended role

- Host reusable graph data structures and algorithms such as topological order,
  strongly connected components, and scheduling helpers.
- Keep graph-specific logic out of IR crates (`signals`, `fir`, `transform`) so
  crate boundaries stay simple.

## Current status

Scaffold only: no graph API, and no workspace crate depends on `graph`.

The port of `compiler/DirectedGraph/` (`Schedule.hh`,
`DirectedGraphAlgorythm.hh`), which the porting plan maps to this crate, lives
in `transform::schedule` instead.

## Public API

| Item | Description |
|---|---|
| `crate_id()` | Returns the stable crate identifier |
