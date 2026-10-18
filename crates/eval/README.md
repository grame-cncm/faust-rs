# eval

Box-level evaluator — Phase 4, section 2.2 of the Faust compiler pipeline.

Takes a parsed box environment and reduces the `process` definition to a fully
evaluated box tree ready for `propagate`.

## C++ provenance

| C++ file | Role |
|---|---|
| `compiler/evaluate/eval.hh` / `eval.cpp` | Core evaluation logic |
| `compiler/evaluate/environment.hh` | Lexical environment model |
| `compiler/evaluate/loopDetector.hh` | Recursive-expansion loop detection |

## What this crate does

1. Builds an `Environment` from the parser's name→expression bindings.
2. Resolves `process`.
3. Evaluates recursively by box family:
   - Lexical forms: `abstr`, `with`, `letrec`, `access`
   - Application: `appl` / `case` (with pattern matching)
   - Iterative forms: `ipar`, `iseq`, `isum`, `iprod`
   - Structural fallback map for non-reducing nodes

## Public API

### Entry points

| Function | Description |
|---|---|
| `eval_process(arena, root)` | Evaluate `process` from a parsed program |
| `eval_entrypoint(arena, root, name)` | Evaluate a named definition instead of `process` |
| `eval_process_with_stats(arena, root)` | Like `eval_process`, also returns `EvalStats` |
| `eval_entrypoint_with_stats(arena, root, name)` | Like `eval_entrypoint`, also returns `EvalStats` |
| `eval_process_with_source_context(arena, root, ctx)` | With source location tracking |
| `eval_entrypoint_with_source_context(arena, root, name, ctx)` | With source location tracking |
| `eval_process_with_stats_and_source_context(arena, root, ctx)` | Stats + source context |
| `eval_entrypoint_with_stats_and_source_context(arena, root, name, ctx)` | Stats + source context |
| `eval_entrypoint_with_source_context_and_cancel(arena, root, name, ctx, token)` | Cooperative cancellation |

### Types

| Item | Description |
|---|---|
| `EvalError` | Typed error covering all evaluation failure modes |
| `EvalStats` | Environment, lookup, node-visit, loop-depth, and definition-name statistics |
| `Environment` | Lexical environment (name → `TreeId`) |
| `EvalSourceContext` | Source-location tracking context passed to evaluator |
| `LoopDetector` | Cycle and depth guard (adapted from C++ `loopDetector` + `stackOverflowDetector`) |
| `SamplePrecision` | `Float32` / `Float64` precision selector |
| `SymId` | Interned symbol identifier |
| `EnvId` | Environment frame identifier |

### Pattern matcher

The case-rule automaton (`make_pattern_matcher`, `apply_pattern_matcher`,
`Automaton`, `State`, `Trans`, `Rule`) lives in the private
`pattern_matcher` module: a port of `compiler/patternmatcher/patternmatcher.cpp`
merged into `eval`. It is not part of the public API.

### Utilities

| Item | Description |
|---|---|
| `crate_id()` | Returns the crate identity string (used for diagnostics) |

## Parity notes

- Non-closure partial application follows C++ `applyList` semantics (implicit wire insertion).
- Loop detection is adapted from C++ `loopDetector` (a periodically checked window of recent
  evaluations) into an exact stack of in-flight `(node or symbol, environment)` frames with
  depth budgets.

## Position in the pipeline

```
parser  →  boxes  →  [eval]  →  propagate  →  signals
```
