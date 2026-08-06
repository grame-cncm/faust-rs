# Memoization Roadmap

This document tracks memoization sites that already exist in `faust-rs` and
the ones that should be added progressively as parity and performance work
continues.

It complements:

- `porting/phases/phase-0-memoization-strategy-en.md`
- `porting/faust-rust-porting-plan-en.md`

The goal here is operational rather than conceptual:

- identify concrete hot paths,
- describe the cache key and cached payload,
- record the expected semantic constraints,
- keep the rollout incremental and testable.

## 1. Rules

Memoization should only be added when all of the following hold:

1. The computation is structurally re-entrant on a DAG and can revisit the same
   node many times.
2. The cached result is stable for an explicit key.
3. The cache boundary can be documented clearly enough that reuse does not hide
   context-sensitive semantics.
4. A structural or differential non-regression test can be added with the
   change.

Preferred Rust pattern:

- keep pass-global caches explicit,
- thread them through one pass/session context,
- do not attach mutable pass state to arena nodes,
- separate analysis caches from operational lowering caches.

## 2. Implemented

### 2.1 `parser`: imported-source expansion cache

Status: implemented

Location:

- `crates/parser/src/source_reader.rs`

Cache:

- `SourceReader.file_cache: HashMap<PathBuf, ExpandedSource>`

Purpose:

- avoids re-reading and re-expanding the same imported Faust file during one
  source-loading session,
- keeps import expansion deterministic while preventing repeated filesystem and
  parser work.

### 2.2 `eval`: loaded-source session cache

Status: implemented

Location:

- `crates/eval/src/lib.rs`

Cache:

- `EvalSourceContext.cache: Arc<Mutex<HashMap<PathBuf, CachedLoadedSource>>>`

Purpose:

- reuses already parsed/loaded source files across `component`/`library`
  evaluation within one evaluator session,
- mirrors the role of the C++ source-reader file cache at the evaluation layer.

Constraint:

- scoped to one `EvalSourceContext`,
- keyed by resolved path, not by raw import string.

### 2.3 `eval`: pattern-matcher automaton cache

Status: implemented

Location:

- `crates/eval/src/lib.rs`
- `crates/eval/src/pattern_matcher.rs`

Cache:

- `LoopDetector.automaton_cache: AutomatonCache`

Purpose:

- memoizes the compiled automaton for one already evaluated `case` rule list,
- avoids recompiling the same effective matcher structure when the same rule
  list is forced multiple times.

Constraint:

- the key is the evaluated rule-list `TreeId`, not the raw syntax tree,
- this is important because lexical evaluation can change the effective rules.

### 2.4 `eval`: symbolic `a2sb` lowering cache

Status: implemented

Location:

- `crates/eval/src/lib.rs`

Cache:

- `LoopDetector.symbolic_box_cache: ahash::HashMap<TreeId, TreeId>`

Purpose:

- memoizes `a2sb(expr)` by original box identity,
- preserves residual-value sharing when the same closure or pattern matcher is
  lowered multiple times in one evaluator session,
- matches Faust C++ `gSymbolicBoxProperty`, which ensures repeated uses of one
  residual value lower to one shared symbolic-slot shape.

Constraint:

- the key is the original pre-lowered `TreeId`, not an arity signature or
  normalized form,
- the cache is session-local because the lowered result depends on the current
  closure/PM side stores and slot-number stream,
- this cache is semantic, not just a speed optimization: without it, repeated
  occurrences of one residual node can allocate fresh slots and silently change
  arity and behavior.

### 2.4b `eval`: expression/environment result cache

Status: implemented

Location:

- `crates/eval/src/lib.rs`

Cache:

- `LoopDetector.eval_cache: ahash::HashMap<EvalCacheKey, EvalValue>`

Purpose:

- memoizes `eval(expr, env)` for one evaluator session,
- mirrors the role of C++ `getEvalProperty(...)` / `setEvalProperty(...)`,
- collapses repeated evaluation of shared higher-order box subgraphs such as
  `jpverb` in `demos.lib`, where the same closure-heavy subtree is revisited
  under the same lexical environment many times.

Constraint:

- the key is the original `TreeId` plus the full lexical environment identity
  (`store`, `env_id`, `source_context`),
- this cache is session-local and must not outlive one evaluation pass,
- because Rust keeps partially applied pattern matchers as host-side values
  with mutable rule-environment state, `EvalValue::PatternMatcher` is
  intentionally not cached yet,
- this is therefore a parity-oriented adapted cache: semantically aligned with
  C++ for first-order boxes and closures, but still narrower than the C++
  tree-property cache because of the current Rust value representation.

### 2.5 `eval`: box simplification cache

Status: **implemented, on the mainline path, and scoped wrongly — this is
currently the single largest compile-time cost in the compiler.** Corrected
2026-08-06; see
`porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md`.

Location:

- `crates/eval/src/simplify.rs` (the function), `crates/eval/src/apply.rs:168`
  (the dominant caller)

Cache:

- `ahash::HashMap<TreeId, TreeId>` threaded through `box_simplification`

Purpose:

- memoizes numeric box simplification on shared box DAGs.

Note (2026-08-06 — the previous note was stale in three ways):

- The path *is* mainline: `apply.rs` calls `box_simplification` on every
  pattern-match dispatch. The `#[allow(dead_code)]` marking is long gone.
- It does **not** mirror `gSimplifiedBoxProperty`. C++ keys that property on
  the tree node and scopes it to the whole compilation; here the caller
  allocates a fresh `HashMap` per dispatch, so the memo's lifetime is one
  argument and every dispatch re-simplifies its subtree from scratch.
- Measured cost: evaluation is 67 % of compile time over the 94-DSP corpus,
  and giving this memo a compilation-scoped lifetime takes the corpus from
  18.1 s to 11.2 s (3.81× → 2.23× vs C++ Faust), with `reverb_designer`
  going 7.2 s → 0.85 s. Rule 1 of §1 is satisfied emphatically; what was
  missing was rule 2's "explicit key" having an explicit *scope*.

### 2.6 `propagate`: box arity cache

Status: implemented

Location:

- `crates/propagate/src/lib.rs`

Cache:

- `ArityCache = AHashMap<FlatBoxId, Result<BoxArity, PropagateError>>`

Purpose:

- avoids repeated arity inference on the same validated flat-box DAG,
- keeps `box_arity*` queries effectively linear on shared subgraphs.

Notes:

- this is an analysis cache,
- it is intentionally kept separate from traversal/lowering memoization.

### 2.7 `propagate`: grouped-UI DAG visitation cache

Status: implemented

Location:

- `crates/propagate/src/lib.rs`

Cache:

- `UiCollector.visited: AHashMap<FlatBoxId, UiCollectSummary>`

Purpose:

- prevents duplicate traversal of shared flat-box subtrees during UI
  extraction,
- avoids ghost controls and duplicated UI ownership artifacts.

### 2.8 `propagate`: De Bruijn lifting and aperture memoization

Status: implemented

Location:

- `crates/propagate/src/lib.rs`

Cache:

- `PropagateMemo.liftn: AHashMap<(TreeId, i64), TreeId>`
- `PropagateMemo.aperture: AHashMap<TreeId, i64>`

Purpose:

- avoids repeated full-subtree rewrites in recursive propagation,
- specifically targets the `liftn` and `aperture` hotspots observed in
  profiling on recursive/shared DAGs.

Context:

- threaded through `PropagateContext`,
- remains local to one propagation traversal.

### 2.9 `normalize`: simplify traversal cache

Status: implemented

Location:

- `crates/normalize/src/simplify.rs`
- `crates/normalize/src/normalform.rs`

Cache:

- `SimplifyCache { nodes: HashMap<SigId, Option<SigId>> }`

Purpose:

- memoizes recursive signal simplification,
- uses `None` as a cycle-breaking sentinel for recursion groups,
- ensures each shared signal node is simplified at most once per pass,
- keeps the cache explicit in Rust while preserving the important behavior of
  the C++ `gGlobal->SIMPLIFIED` tree property.

Scope:

- `simplify(...)` still allocates a fresh `SimplifyCache` for one standalone
  signal root,
- `simplify_signals_fastlane(...)` now allocates one `SimplifyCache` for the
  whole prepared output forest and threads it through every output root,
- on a caught simplification panic, `simplify_signals_fastlane(...)` clears the
  cache before returning the original root for that output, so no partial
  traversal state is reused after unwinding.

Why the forest scope matters:

- C++ `simplify(Tree sig)` stores results directly on tree nodes via
  `SIMPLIFIED`, so repeated calls over shared roots reuse previous
  simplification results,
- the initial Rust port created a fresh `HashMap` for every output in
  `simplify_signals_fastlane(...)`; large RAD/FAD-expanded DSPs with many
  related outputs could therefore redo the same `sig_map` and
  `normalize_add_term` work across the forest,
- a macOS `sample` on `rad_fxlms1.dsp` showed the active worker dominated by
  `normalize::simplify::sig_map`, `Aterm::add_sig`,
  `normalize_add_term`, `greatest_divisor`, and `mterm::gcd`, matching this
  missing cross-root reuse pattern.

Semantic constraints:

- the cache key is the canonical `SigId` in one `TreeArena`,
- the cached value is valid only for the same `SigType` map and simplification
  pass,
- typed and untyped simplification must not share one cache,
- the cache is not stored in `TreeArena`; callers choose the pass boundary
  explicitly.

Validation:

- `simplify_with_cache_reuses_seen_root` checks that a repeated root reuses the
  same cache entries,
- `rad_fxlms1.dsp` with `N = 512` compiled through the patched release
  `faust-rs` in about 1.6 seconds after this cache was shared across the output
  forest.

### 2.10 `normalize`: promotion cache in normal-form pipeline

Status: implemented

Location:

- `crates/normalize/src/normalform.rs`

Cache:

- `SignalPromoter.memo: HashMap<SigId, SigId>`

Purpose:

- memoizes only the context-free reconstruction `promote(sig)` during
  normal-form preparation,
- preserves sharing while inserting only the required casts,
- stays sound because parent-owned integer/real coercions (`select2`,
  delay/table indices, `enable`, `wrtbl` writes, mixed arithmetic) are applied
  outside the cache via explicit helpers.

Note:

- this cache no longer lives in `transform::signal_prepare`; the fast-lane
  consumes the shared promotion pass from `normalize`,
- the cache is intentionally *not* context-tagged: remaining memoized results
  are justified as context-invariant after the node-wise C++ parity refactor.

### 2.11 `transform`: reduced type inference state for prepared signals

Status: implemented

Location:

- `crates/transform/src/signal_prepare.rs`

Memoized state:

- `node_types: HashMap<SigId, TypeSlot>`
- `group_types: HashMap<SigId, Vec<TypeSlot>>`
- `active_groups: HashMap<SigId, Vec<TypeSlot>>`

Purpose:

- memoizes reduced typing over symbolic recursion groups,
- stores both final node/group results and temporary recursion-group fixpoint
  state.

Note:

- this is not a simple lookup cache; it is still memoized analysis state and
  should be tracked as such.

### 2.12 `transform`: signal-to-FIR lowering DAG cache

Status: implemented

Location:

- `crates/transform/src/signal_fir/module.rs`

Cache:

- `SignalToFirLower.cache: HashMap<SigId, FirId>`

Purpose:

- memoizes already lowered FIR expressions for shared signal DAG nodes,
- prevents duplicate FIR subgraphs and keeps lowering linear in the shared
  graph size.

### 2.13 `transform`: unary symbolic recursion discovery visitation set

Status: implemented

Location:

- `crates/transform/src/signal_prepare.rs`

Memoized state:

- `HashSet<SigId>` threaded through `collect_unary_sym_groups(...)`

Purpose:

- memoizes traversal reachability while discovering unary symbolic recursion
  groups during `prepare_signals_for_fir(...)`,
- ensures each shared signal node is analyzed at most once for this discovery
  phase,
- prevents exponential revisitation on shared DAGs such as
  `dsp/cubic_distortion.dsp`.

Constraint:

- this is traversal-state memoization, not a semantic result cache,
- it is scoped to one preparation forest and only guards the read-only
  discovery walk that populates the unary-group map.

### 2.14 `tlib`: de Bruijn recursion conversion memos

Status: implemented

Location:

- `crates/tlib/src/recursion.rs`

Caches:

- `convert_memo: AHashMap<TreeId, TreeId>`
- `substitute_memo: AHashMap<(TreeId, i64, TreeId), TreeId>`
- `aperture_memo: AHashMap<TreeId, i64>`
- additional `(TreeId, i64) -> TreeId` memo for recursive lifting helpers

Purpose:

- preserves graph sharing while converting de Bruijn recursion to symbolic
  recursion,
- avoids repeated substitution and aperture queries on shared recursive trees.

## 3. Planned Additions

The items below are ordered by expected leverage and safety.

### 3.1 `propagate`: memoize propagation of context-free closed subtrees

Status: planned

Target:

- `crates/propagate/src/lib.rs`

Likely cache shape:

- `AHashMap<(FlatBoxId, Vec<SigId> or specialized key), Vec<SigId>>`
- or preferably a narrower cache only for proven closed subtrees

Why:

- `propagate_inner` still recomputes some subtrees that do not depend on
  `slot_env`, `clock_env`, or dynamic input slicing.

Constraint:

- do not cache general `propagate_inner` results blindly,
- only cache subtrees whose output is provably independent of dynamic context.

Validation:

- structural tests on recursion and clocked wrappers,
- targeted profile before/after on shared recursive DSPs.

### 3.2 `normalize`: broader normal-form stage caching beyond local simplify/promote passes

Status: planned

Target:

- `crates/normalize`
- possibly helper caches in `crates/signals`

Likely cache shape:

- `AHashMap<SigId, SigId>` or a small staged cache bundle owned by the
  normal-form coordinator

Why:

- the local simplify and promotion passes are already memoized,
- `simplify_signals_fastlane(...)` now shares its local simplify cache across
  one prepared output forest,
- but the overall normal-form pipeline still has room for a more explicit
  staged cache strategy when multiple normalization sub-passes are chained.

Constraint:

- cache keys must reflect the exact sub-pass and typing mode,
- avoid mixing typed and untyped normalization results in one cache.

Validation:

- differential tests against C++ simplification-sensitive corpus cases,
- idempotence tests: `normalize(normalize(x)) == normalize(x)`.

### 3.3 `transform`: recursion / cycle marking cache

Status: planned

Target:

- `crates/transform`

Likely cache shape:

- `AHashMap<SigId, bool>`
- or `HashSet<SigId>` plus an in-progress mark set

Why:

- recursive analyses in scheduling/FIR lowering should not rediscover the same
  cycle structure repeatedly.

Constraint:

- distinguish memoized final state from temporary DFS visitation state,
- document precisely whether the cache means “is recursive”, “can reach
  recursion”, or “already fully explored”.

Validation:

- recursion-heavy FIR structural tests,
- no false positives on acyclic shared graphs.

### 3.4 `codegen`: signal occurrence counting cache

Status: planned

Target:

- `crates/codegen`

Likely cache shape:

- `AHashMap<SigId, usize>`

Why:

- variable scheduling and temporary materialization depend on how many times a
  node is consumed,
- repeated recounting over shared DAGs is wasteful.

Constraint:

- counts must be defined for the exact scheduling scope,
- do not reuse counts across different backend-specific traversal policies.

Validation:

- structural backend tests for temporary emission,
- parity checks on representative shared-expression corpus cases.

### 3.5 `codegen` / runtime lowering: computed delay cache

Status: planned

Target:

- `crates/codegen`
- possibly `crates/transform` depending on ownership of delay analysis

Likely cache shape:

- `AHashMap<SigId, usize>`

Why:

- recursive delay computation is reused by memory layout and runtime lowering.

Constraint:

- cache semantics must be tied to one precise delay notion,
- do not mix “minimum delay”, “maximum delay”, and “buffer size” in the same
  cache.

Validation:

- delay-line allocation tests,
- differential runtime checks on delay-heavy corpus cases.

### 3.6 `propagate`: route flattening cache

Status: opportunistic

Target:

- `crates/propagate/src/lib.rs`

Likely cache shape:

- `AHashMap<TreeId, Vec<i64>>`

Why:

- `flatten_route_ints` is pure and easy to cache.

Constraint:

- lower expected payoff than the items above,
- only worth adding if profiling shows repeated route decoding.

## 4. Explicit Non-Goals

These are not good general-purpose memoization candidates unless profiling and
semantics clearly justify them:

- `eval` deep reduction with an implicit `(Tree, Environment)` cache key,
- fully generic `propagate_inner` caching across arbitrary input/context state,
- tiny tag-decoding helpers where the cost is dominated by larger traversals,
- caches that silently merge results from different precision, typing, or
  backend modes.

## 5. Rollout Discipline

For each new memoization site:

1. Add one local explanation in code near the cache definition.
2. Document the key and invalidation boundary in Rustdoc or nearby comments.
3. Add at least one non-regression test.
4. Prefer one cache at a time, not large speculative cache batches.
5. Re-check that the new cache does not accidentally replace a clearer
   higher-level context boundary.

## 6. Current Priority

Reordered 2026-08-06 on measured evidence rather than expectation
(`porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md`):

1. **`eval`: give the existing `box_simplification` memo a compilation-scoped
   lifetime (§2.5).** Not a new memoization — a scope fix on one that already
   exists. Worth ~6.9 s of the corpus's 18.1 s; nothing else measured comes
   close.
2. `propagate`: cache only provably context-free closed subtree propagation.
   Note the `propagation` stage is 2.2 % of compile time, so this is bounded by
   ~0.4 s on the corpus however well it is done.
3. `normalize`: introduce a signal normal-form cache.
4. `codegen`: add occurrence counting cache once the scheduling path is stable.

The reordering is the point: items 2-4 were listed first for two years on
plausibility. The measurement says the largest available win was a scope bug in
an already-implemented cache that the roadmap recorded as inactive.
