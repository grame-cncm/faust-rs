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
| `compiler/boxes/ppbox.hh` / `ppbox.cpp` | Faust-syntax box printers (`boxpp`, `boxppShared`) |
| `compiler/boxes/boxcomplexity.cpp` | Box complexity used by SVG folding |

## Public API

| Item | Description |
|---|---|
| `BoxBuilder` | Construction API — 1:1 mapping with C++ `box*` constructors, plus faust-rs extensions (`fad`/`rad`, `cinputs`/`coutputs`) |
| `BoxMatch` / `match_box` | Pattern-matching API — structural inspection of a box node |
| `BoxId` | `TreeId` alias for box nodes |
| `dump_box` | Debug pretty-printer for a box subtree |
| `box_pp(arena, node, priority, float_size)` | Faust-syntax expression of one box, shared subtrees expanded |
| `box_pp_shared(arena, root, float_size)` | Faust-syntax program with one `ID_<n>` definition per shared node (the `-e` printer), returns `SharedBoxProgram` |
| `format_real` / `FloatSize` / `BoxPrintError` | Real-literal formatting, float precision, and printer error |
| `box_complexity(arena, box)` | Box complexity score used by `draw` folding |

## Parity invariants

- Box nodes are represented as tagged trees with **deterministic child order**.
- Labels and identifiers are carried as `NodeKind::Symbol`.
- UI slider/numentry parameter payload keeps Faust list encoding: `list4(cur, min, max, step)`.
- Public integer surface is `i32`-based (`boxInt` parity); storage remains `NodeKind::Int(i64)`.
- `BoxBuilder` now keeps lightweight debug-only child-existence assertions on
  key composite constructors so structural mistakes fail close to the builder boundary.

## Position in the pipeline

Canonical pipeline (AGENTS.md): parse → boxes → eval → propagate → normalize → type/interval → transform → fir → backend.

Box trees are built by `parser`, rewritten by `eval`, and consumed by
`propagate` (box → signal) and by `draw` (SVG diagrams).
