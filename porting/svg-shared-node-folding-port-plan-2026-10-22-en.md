# Port of C++ #1290 to `draw`: shared nodes folded in `-svg` drawings

**Date:** 2026-10-22
**C++ reference:** `master-dev` `6789c5da8` (#1290, 2026-09-25), with the
label fix `068fb9a80` (2026-10-22). Both are newer than the pinned
reference `8eebea429` (2.84.3); the `-svg` behaviour below follows them.

## 1. What #1290 changed, and where faust-rs stands

| #1290 part | C++ | faust-rs before this port |
|---|---|---|
| `applyList` memo | `gApplyMemo` | already ported (`porting/MEMOIZATION.md` §2.4e): `cheb(30)` compiles in milliseconds |
| `boxComplexity` | memoized as a tree property, now saturating at 10^9 | recomputed over the expanded tree at every call, not saturating |
| shared nodes in `-svg` | `Occur` counts the parents of each node of the DAG; a shared node is folded even without a name | only named nodes fold; a shared unnamed node is expanded as a tree |
| label of a folded shared node | `diagram_N`, then (since `068fb9a80`) its own expression printed within a budget | — |

Measured on `cheb(n,x) = 2*x*cheb(n-1,x) - cheb(n-2,x)`, `process =
cheb(23)` (no library), `-svg`:

| compiler | time | memory | output |
|---|---|---|---|
| C++ 2.84.3 (pinned) | 7.1 s | 1.2 GB | 431 MB, 1 file |
| C++ 2.90.14 (#1290) | 0.00 s | 7 MB | 264 KB, 22 files |
| faust-rs 0.9.2 | 1.3 s | 543 MB | 373 MB, 22 files |

The output grows like the Fibonacci sequence: cheb(30) would be about 10 GB.

## 2. Design

1. **`boxes::BoxComplexity`**, a memo of `box_complexity` per `BoxId`
   (lifetime: one drawing), saturating at `MAX_BOX_COMPLEXITY = 10^9` as
   C++. `box_complexity(arena, b)` keeps its signature and uses a fresh
   memo, so it becomes linear in the DAG instead of the expanded tree.
2. **Parent counts.** At the start of a drawing, one walk over the DAG with
   a visited set counts, for every node, the edges from distinct parents
   (C++ `Occur::countOccurrences`, children of every node kind, the root
   not counted). A node is *shared* when its count exceeds 1.
3. **Fold condition** (`generate_diagram_schema`), as C++
   `generateDiagramSchema`: folding on, complexity ≥ `fold_complexity`, and
   (named or shared). A node already scheduled is linked again, not
   expanded: Rust inlined a second occurrence of a folded named node
   (`!drawn.contains(&b)` in the condition), which C++ never did, and which
   would re-expand every shared node.
4. **Label of an unnamed shared node:** its expression, `boxes::box_pp`
   under a node budget (`box_pp_bounded`, C++ `mBox`/`PrintBudget`, budget
   1024), shortened by `max_name_size` like any name. Its file stem is
   `diagram` (`diagram-<hex>.svg`, or `diagram.svg`, `diagram-2.svg` under
   `FileNaming::DefinitionName`): an expression is not a file name.
5. **Arity of a folded node without expanding it.** The link block needs
   the node's inputs and outputs. Rust read them from the *unfolded*
   inside schema, itself exponential on a shared DAG; C++ calls
   `getBoxType`. The folded inside schema of the node is built once, when
   it is first scheduled (with the node's own file as the current file, so
   its children link back to it), its arity is kept per `BoxId`, and the
   schema is reused when the node's file is drawn. Each node's inside is
   generated once.

## 3. Deliverables and pass criteria

- `cheb(23)` and `cheb(30)` draw in well under a second, output under
  1 MB, through `faust-rs -svg` and `faustprobe --svg`.
- Every program of `tests/impulse-tests/dsp` still draws; file count and
  size reported before/after; two runs byte-identical (`faustprobe`).
- Tests: `boxes` (memo equals the unmemoized definition, saturation,
  bounded print), `draw` (a shared unnamed node folded once and linked at
  every occurrence, a named node drawn once and linked twice, output
  linear in the DAG on a Fibonacci-shaped DAG), `faustprobe` (`cheb(30)`
  `--svg` fast and small).
- `porting/MEMOIZATION.md` §2 entry for the complexity memo and the
  prepared schemas; difference registry and journal updated.
