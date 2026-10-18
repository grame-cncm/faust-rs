# doc

Mathematical documentation crate — scaffold placeholder.

## Intended role

The porting plan maps the C++ automatic mathematical documentation generator,
`compiler/documentator/` (the `-mdoc` option: `doc.cpp`, `doc_compile.cpp`,
`lateq.cpp`, …), to this crate.

## Current status

Not ported: the crate is an empty scaffold, no workspace crate depends on it,
and the `faust-rs` CLI has no `-mdoc` option.

## Public API

| Item | Description |
|---|---|
| `crate_id()` | Returns the stable crate identifier |
