# Missing Features / Known Gaps

This file documents features or passes that exist in the C++ reference compiler but are
not yet implemented in faust-rs, along with the concrete symptoms they produce.

Each entry points to the relevant porting plan (when one exists) and lists affected test
cases so that progress can be verified once the feature is implemented.

---

## 1. Constant-folding pass (`normalize/simplify.cpp`) — closed

Status: closed on 2026-08-11. This section retains the original failure and
its resolution for provenance; it is no longer a known gap.

### C++ location
`compiler/normalize/simplify.cpp` — the `simplify(Tree sig)` function, called as part of
`simplifyToNormalForm()` before any backend sees the signal tree.

### What it does
A bottom-up memoised walk of the entire signal tree that rewrites:
- `BinOp(op, Int(a), Int(b))` → `Int(op(a,b))` (and the float equivalent)
- Various algebraic identities: `n*(m*x)` → `(n*m)*x`, `-1*(x-y)` → `y-x`, etc.
- Calls `computeSigOutput` on `xtended` (primitive math) nodes with constant arguments.

The net effect: **all constant arithmetic is fully reduced to a single `Int` or `Real`
node before code generation.**

### Resolution
`signal_prepare` now runs the canonical bottom-up simplifier over the complete
promoted signal forest. Consequently table-size children are reduced before
the scalar or checked-vector FIR table extractor sees them. The maintained
`rep_87_table_computed_size.dsp` fixture checks scalar/vector and runtime/const
code generation, while `corpus-runtime-diff` checks three numerical scenarios
against the pinned C++ interpreter output.

### Concrete symptom — `tabulateNd_test`

**Test**: `ba.tabulateNd(1, powSin, (8,8, 2.0,2.0, 8.0,8.0, 3.0,4.0)).lin`
**Former error**: `[FRS-SFIR-0004] SIGWRTBL currently requires constant integer size in Step 2H`

**Former root cause**: `tabulateNd` computes the total table size as `tableSize = size(N)` where
`size(2) = _ * size(1) = _ * _` — a multiplication of the two dimension sizes.  With
inputs `(8, 8)` this becomes `BinOp(Mul, Int(8), Int(8))` in the signal tree.  The C++
`simplify` pass folds this to `Int(64)` before `generateTable` is called; faust-rs did
not, so `table_size_from_sig` saw a non-`Int` node and rejected it.

The reference C++ output confirms a `float ftbl0mydspSIG0[64]` static array — the size
is a compile-time constant `64`.

**Affected tests** (basics_tests.dsp):
- `tabulateNd_test`

The implemented fix is global simplification, not a table-local evaluator. The
`8*8` expression becomes `Int(64)` before both FIR lowerers, matching C++
`simplifyToNormalForm()`.
