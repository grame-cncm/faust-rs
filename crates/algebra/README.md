# algebra

Algebra support crate — scaffold placeholder.

## Intended role

- Planned home of reusable algebraic identities and canonicalization helpers
  shared by compiler passes.
- Avoid circular dependencies between IR crates by keeping this crate focused on
  algebraic utilities rather than owning a full IR.

## Current status

Scaffold only: no public rewrite API, and no workspace crate depends on
`algebra`.

## C++ provenance

The porting plan maps `compiler/FaustAlgebra/FaustAlgebra.hh` to this crate.
That header is an abstract per-primitive algebra (dispatch tables over the
signal primitives) that the C++ interval algebra (`interval/interval_algebra.hh`)
derives from. It is not ported as such: the Rust interval operators are free
functions in `interval::ops`, called by the typing rules of `sigtype`.
Algebraic simplification proper lives in `normalize`.

## Public API

| Item | Description |
|---|---|
| `crate_id()` | Returns the stable crate identifier |
