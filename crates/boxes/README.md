# boxes

Box-expression construction and pattern-matching helpers backed by `tlib::TreeArena`.

Boxes are the Faust compiler's mid-level representation between parsing and signal
propagation.  Every source-level expression (composition operators, primitives, UI
widgets, foreign calls, …) is lowered into a box tree before the evaluator runs.

## C++ provenance

| C++ file | Role |
|---|---|
| `compiler/boxes/boxes.hh` | Box node families and constructors |
| `compiler/boxes/boxes.cpp` | Construction / inspection implementation |

## Public API

| Item | Description |
|---|---|
| `BoxBuilder` | Construction API — 1:1 mapping with C++ `box*` constructors, plus faust-rs extensions (`fad`/`rad`, `cinputs`/`coutputs`) |
| `BoxMatch` / `match_box` | Pattern-matching API — structural inspection of a box node |
| `BoxId` | `TreeId` alias for box nodes |
| `dump_box` | Debug pretty-printer for a box subtree |

## Parity invariants

- Box nodes are represented as tagged trees with **deterministic child order**.
- Labels and identifiers are carried as `NodeKind::Symbol`.
- UI slider/button parameter payload keeps Faust list encoding: `list4(cur, min, max, step)`.
- Public integer surface is `i32`-based (`boxInt` parity); storage remains `NodeKind::Int(i64)`.
- `BoxBuilder` now keeps lightweight debug-only child-existence assertions on
  key composite constructors so structural mistakes fail close to the builder boundary.

## Position in the pipeline

```
parser  →  [boxes]  →  eval  →  propagate  →  signals  →  transform  →  codegen
```
