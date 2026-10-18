# eval

Box-level evaluator — Phase 4, stage 4.2 of the porting plan.

Takes a parsed box environment and reduces the `process` definition to a fully
evaluated box tree ready for `propagate`.

## C++ provenance

| C++ file | Role |
|---|---|
| `compiler/evaluate/eval.hh` / `eval.cpp` | Core evaluation logic |
| `compiler/evaluate/environment.hh` / `environment.cpp` | Lexical environment model |
| `compiler/evaluate/loopDetector.hh` / `loopDetector.cpp` | Evaluation loop and stack-depth detection |
| `compiler/patternmatcher/patternmatcher.hh` / `patternmatcher.cpp` | Case-rule automaton, merged into `eval` (`pattern_matcher.rs`) |

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
| `eval(arena, definitions, &request) -> Result<(TreeId, EvalStats), EvalError>` | Evaluate an entry point described by an `EvalRequest` |
| `eval_process(arena, definitions) -> Result<TreeId, EvalError>` | Shorthand: `process`, in memory, uncancellable, statistics dropped |
| `eval_box(arena, expr, &env, &mut loop_detector) -> Result<TreeId, EvalError>` | Evaluate one box expression in an explicit environment |

`EvalRequest::default()` evaluates `process` with an in-memory source context;
`with_entrypoint(name)`, `with_source_context(ctx)` and `with_cancel(flag)`
select another entry point, file resolution for `component`/`library`, and
cooperative cancellation.

### Types

| Item | Description |
|---|---|
| `EvalRequest` | Entry point, source context and cancellation flag of one run |
| `EvalError` | Typed error covering all evaluation failure modes |
| `EvalWarning` | Non-fatal finding (unmatched modulation target) |
| `EvalStats` | Warnings, definition names, composition origins, and partial environment counters |
| `DepthGuard`, `DepthLimit`, `DepthLimitSource`, `DepthContext` | Depth-budget diagnostics |
| `Environment` | Lexical environment (name → box, closure or pattern-matcher value) |
| `EvalSourceContext` | Source-resolution context for `component` / `library` loading |
| `LoopDetector` | Cycle and depth guard (adapted from C++ `loopDetector` + `stackOverflowDetector`) |
| `SamplePrecision` | `Float32` / `Float64` precision selector |
| `SymId` | Interned symbol identifier |
| `EnvId` | Environment frame identifier |

### Suggestions

The public `suggestions` module (`SymbolSuggestion`, `rank_similar_names`,
`unambiguous_suggestion`, also re-exported at the crate root) ranks near names
for undefined-symbol diagnostics.

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

Canonical pipeline (AGENTS.md): parse → boxes → eval → propagate → normalize → type/interval → transform → fir → backend.

`eval` already depends on `propagate` and `normalize` for constant folding: a
closed box is propagated to signals (`propagate::propagate_fold` /
`propagate_typed`) and simplified (`normalize::try_simplify_const`).
