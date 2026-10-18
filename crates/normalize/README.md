# normalize

Signal normalization and algebraic simplification — ported from `compiler/normalize/`.

## C++ provenance

| C++ path | Role |
|---|---|
| `compiler/normalize/normalize.hh` / `normalize.cpp` | Add-term and delay-term normalization |
| `compiler/normalize/simplify.hh` / `simplify.cpp` | Memoized signal rewrite engine |
| `compiler/normalize/normalform.hh` / `normalform.cpp` | Normal-form pipeline coordinator |
| `compiler/normalize/aterm.*` / `mterm.*` | Additive and multiplicative terms |
| `compiler/transform/sigPromotion.cpp` | `SignalPromotion` (casts) and `SignalTablePromotion` (`-ct`) |

## Architecture

The normalization modules follow a five-layer dependency order:

```
normalform   ← promotion, simplification entry points, normal-form preparation
  simplify   ← memoized rewrite engine
    normalize  ← add-term + delay-term normalization
      aterm    ← additive term (sum of mterms)
        mterm  ← multiplicative term (k · x^n · y^m / …)
```

Two further passes stand beside them: `rec_merge` (merges isomorphic `SYMREC`
groups; no C++ counterpart) and `table_promote` (`-ct` table index clamping).

## Public API

| Item | Description |
|---|---|
| `normalform::prepare_signals(arena, ui, sig, opts)` | Normal-form preparation for one signal (de-Bruijn → symbolic → typed → promoted) |
| `normalform::prepare_signals_multi(arena, ui, sigs, opts)` | Multi-output variant of `prepare_signals` |
| `normalform::promote_signals(arena, types, sigs)` | Signal promotion pass using canonical inferred types |
| `normalform::promote_signals_fastlane(arena, types, sigs)` | Fallible promotion pass used by `transform::signal_prepare` |
| `normalform::simplify_signals_fastlane(arena, types, sigs)` | Typed algebraic simplification used by `transform::signal_prepare` |
| `normalform::NormalFormOpts` | Options controlling the preparation pipeline |
| `normalform::NormalFormError` | Typed error covering recursion and type failures |
| `simplify_const(arena, sig)` | Fold constant sub-expressions in a signal tree |
| `try_simplify_const(arena, sig)` / `try_simplify_const_cached(arena, cache, sig)` | Same, returning `DivisionByZero` instead of unwinding (used by `eval`); `ConstSimplifyCache` keeps the memo across calls |
| `merge_isomorphic_symrec_groups(arena, outputs)` | Merge structurally isomorphic `SYMREC` groups |
| `promote_table_signals(arena, types, sigs, warnings)` | Clamp table indexes the intervals cannot prove in bounds (`-ct`), reporting `TableRangeWarning`s |
| `crate_id()` | Returns the stable crate identifier |

## Position in the pipeline

Canonical pipeline (AGENTS.md): parse → boxes → eval → propagate → normalize → type/interval → transform → fir → backend.

`normalize` is a library, not a standalone pass: `transform::signal_prepare`
calls its promotion, simplification, `rec_merge` and `table_promote` passes
between re-typings with `sigtype` (on which `normalize` depends), and `eval`
uses its constant folding. `prepare_signals*` is used by the signal C API.
