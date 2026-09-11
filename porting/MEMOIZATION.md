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
5. **The repeat rate is measured, not assumed** — on an input where the stage
   in question dominates.
6. **The key names everything the value depends on.** A node identity is not a
   context: the same `SigId` means one thing inside a clock domain and another
   in its sibling (§2.12d), one thing inside a recursion binder and another
   outside it (§2.17), one class at one de Bruijn level and another at the next
   (§2.18). A key that is too coarse is not slow, it is wrong, and no test that
   only checks compile time will see it.
7. **Count the cost of the key itself.** A key computed by a walk that is
   exponential where the graph is shared can cost more than everything the cache
   saves: §2.19 spent five minutes and ten gigabytes building keys before
   lowering a single instruction, and the fix was the key function, not the
   cache.

Rules 6 and 7 were added 2026-09-11, after a sweep of the sites added since
June found that both had been learned the expensive way.

Rule 5 was added 2026-08-06 because rules 1–4 only test whether a computation
*could* be memoized. §3.1 satisfied all four and turned out to be a
pessimization: 12 % hit rate, 748 k entries stored to avoid 123 k
recomputations. Choose the input deliberately too — the impulse corpus puts
propagation at 2.2 % of compile time where a real DSP puts it at 82 %.

Preferred Rust pattern:

- keep pass-global caches explicit,
- thread them through one pass/session context,
- separate analysis caches from operational lowering caches,
- **and pick the owner by what the cached value depends on, not by habit.**

That last point replaces a flat "do not attach mutable pass state to arena
nodes" (2026-08-06). The prohibition is right about *pass state* — a value that
depends on where the pass currently is must not be parked on a node shared by
every pass. It is wrong about a value that is a pure function of the node
itself: for those the arena is the *correct* owner, because a `TreeId` is only
meaningful to the arena that issued it, so arena ownership makes the memo's
lifetime and its keys expire together and removes invalidation as something
anyone has to remember. That is what C++ does through `CTree::setProperty`, and
it is how §2.5 was fixed. The test that distinguishes the two cases is whether
a fresh arena must see an empty table — if yes, the arena should own it.

## 2. Implemented

Entries are numbered in the order they were written down, not by crate: a
letter suffix marks a sibling of an existing entry, a new number an area the
document had not covered. The inventory was last swept against the code on
2026-09-11, over every commit since 2026-06-22.

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

### 2.1b `parser`: remote import cache

Status: implemented 2026-08-11 (`6be7e164`)

Location:

- `crates/parser/src/source_reader.rs`, in `read_locator`

Cache:

- `SourceReader.remote_cache: HashMap<Url, (Arc<str>, Url)>`, the remote
  sibling of §2.1's field on the same struct,
- the payload is the fetched source and the **final** URL, the one reached after
  redirects.

Purpose:

- one network fetch per URL per session, where a repeat is not merely slow but
  non-deterministic,
- **redirect aliasing**: every successful fetch inserts two entries, under the
  requested URL and under the final one, so a redirect target is canonical and
  cannot slip past the reader's duplicate and cycle detection, nor change the
  base of relative imports.

Constraints:

- failures are not cached: the fetch and the UTF-8 decode both return before any
  insert, so a failing URL is retried at each occurrence,
- the cache is one parse session wide and networking exists only if the host
  installed a fetcher through `with_remote_fetcher`.

Validation:

- `crates/parser/tests/api_bridge.rs` (four remote cases),
  `crates/compiler/tests/remote_fetch_transport.rs` (eight),
- gap: no test counts fetches, so the fetch-once property itself is untested;
  the redirect alias is covered only indirectly.

### 2.1c `parser`: process-wide lexer definition and combined DFAs

Status: implemented 2026-08-06 (`0691179e`)

Location:

- `crates/parser/src/lib.rs`: `LEXERDEF` and `DFAS`, two `std::sync::OnceLock`s

Purpose:

- `lexerdef()` compiles the 128 rules of `faustlexer.l` into automata in 2.3 ms
  and used to be called **once per file**: a two-line DSP importing
  `stdfaust.lib` lexes 453 836 bytes over ten files, so it rebuilt the same
  constant ten times, 23 ms of a 249 ms compile.

Constraint:

- these are the one kind of cache the §1 rules do not govern: the value has no
  key and depends on nothing a compilation owns. It is sound because the
  definition is immutable and every piece of mutable state — position, the
  `comment` / `doc` / `lst` start conditions — lives in the lexer the definition
  hands back.

Validation:

- `cargo run --release -p parser --example lexbench` is kept as the evidence.

### 2.2 `eval`: loaded-source session cache

Status: implemented

Location:

- `crates/eval/src/source_context.rs`

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

### 2.4c `eval`: normal-form fast path

Status: implemented 2026-09-10 (`2616786f`)

Location:

- `crates/eval/src/normal_form.rs` (the predicate)
- `crates/eval/src/lib.rs`, the two call sites in `eval_value`

Caches:

- `LoopDetector.normal_form_cache: ahash::HashMap<TreeId, bool>`
- `LoopDetector.evaluated_boxes: ahash::HashSet<TreeId>`

Purpose:

- a tree the evaluator has itself produced and that is in normal form — numbers,
  wires, cuts, primitives, waveforms, widgets whose label carries no `%`
  variable and whose parameters are in normal form, and the compositions of
  those — evaluates to itself in every environment, so `eval_value` returns it
  without walking it and without writing an entry per environment layer,
- this removes the cubic cost of `ba.take` over a long list: in
  `take(n, (x, xs)) = take(n - 1, xs)` the variable `xs` is bound to the tail of
  the list, already evaluated, and every recursion level is a fresh layer that
  §2.4b's `(expr, env)` key has never seen, so the tail was walked whole at
  every level, `O(n)` per level, `O(n³)` in all.

Key:

- the verdict is keyed by `TreeId` alone, because "is in normal form" is a pure
  function of the tree; by §1 the arena would be its natural owner, and the
  table sits on the session's `LoopDetector` only because the companion set —
  which trees the evaluator produced — is session state,
- the predicate walk is iterative: a list of a thousand elements is a tree a
  thousand levels deep.

Constraints, both found by failing tests rather than by reasoning:

- a **source** tree never takes the path, whatever its shape: it still has its
  constants to fold and it must still be counted against the evaluator's nesting
  budget,
- a `:` whose left side is a numerical tuple is not a fixed point: applying a
  primitive to constants yields `(20 : log)` unfolded and relies on its next
  evaluation to fold it, which the seed of a `fad` in `descend_1D` depends on.
  While that exclusion was missing, the tutorial's `s05_2` and `s07_2` stopped
  learning, seed and occurrence no longer being the same tree,
- identifiers, applications, abstractions, closures, `case`, `with`, `letrec`,
  iterations, accesses, components, metadata, routes, foreign declarations,
  clock-domain blocks and the AD primitives are excluded by construction.

Validation:

- `crates/compiler/tests/eval_normal_form.rs`: a 480-element `ba.take`, the sum
  of the list checked,
- 480 elements go from more than two minutes to 1.0 s and 1090 elements compile
  in 3.1 s, where the C++ compiler takes 6.9 s; goldens, `expand_corpus` and the
  tutorial's learning programs are unchanged.

Note: this is a workaround for a representation difference, not parity. C++
hash-conses its environment layers, so its `(tree, env)` cache hits across
recursion levels; ours allocates a fresh layer per level. `propagate` has since
canonicalized its own contexts (§2.16); doing the same in `eval` is §3.7.

### 2.4d `eval`: closure interning

Status: implemented 2026-08-30 (`db7a0668`)

Location:

- `crates/eval/src/loop_detector.rs`, `LoopDetector::store_closure`

Cache:

- `closure_intern: ahash::HashMap<(TreeId, EnvFrameKey), i32>`, the dense index
  into the existing `closure_store: Vec<ClosureValue>` — the integer carried by
  the `boxClosure(boxInt(key))` node.

Purpose:

- the key is exactly the equality `ClosureValue` already defines, expression
  plus environment identity. Before this, `store_closure` minted a fresh key per
  call, so *n* re-evaluations of one residual abstraction produced *n* distinct
  handles; `a2sb` then lowered each to a *fresh* slot, and propagation saw *n*
  structurally identical bodies under *n* distinct slot environments,
- C++ has no such mode because `closure(...)` is a hash-consed tree node: equal
  closures *are* one node. This restores that identity in the Rust handle
  representation, so it is hash-consing parity rather than caching of a
  computation.

Effect on the other caches:

- §2.4's `symbolic_box_cache` now yields one slot per residual abstraction
  instead of one per occurrence, §2.4b hits more often, and §2.15's key stops
  seeing one fresh slot environment per miss.

Constraint:

- the key holds a raw store pointer; the stored `ClosureValue` clone keeps the
  environment's `Rc` alive, so a pointer inside a live key cannot be recycled.

Validation:

- on `virtualAnalog.dsp`: propagation calls 974 935 to 52 963 (C++ about 8 600),
  eval 0.89 s to 0.47 s, propagation 0.89 s to 0.66 s, total 2.22 s to 1.54 s,
  output byte-identical; over `examples/`, the ratio against C++ goes from 0.74×
  to 0.63×,
- gap: no dedicated test was added; the path is exercised indirectly by the
  residual-closure cases of `crates/eval/tests/core_eval.rs`.

### 2.5 `eval`: box simplification cache

Status: **implemented, on the mainline path, arena-scoped since 2026-08-06.**
See `porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md`.

Location:

- `crates/eval/src/simplify.rs` (the function),
  `crates/eval/src/pattern_matcher.rs` (the callers today; in 2026-08 it was
  `apply.rs`, on every pattern-match dispatch — see the history below)

Cache:

- `PropertyStore<TreeId>` owned by `TreeArena`, under the `simplified-box`
  property key — the Rust shape of C++ `CTree::setProperty` /
  `gGlobal->gSimplifiedBoxProperty`.

Purpose:

- memoizes numeric box simplification on shared box DAGs, once per
  compilation.

History (worth keeping — the failure was subtle and expensive):

- Until 2026-08-06 the cache was an `ahash::HashMap` supplied by the caller,
  and the dominant caller — `apply.rs`, on every pattern-match dispatch —
  allocated a fresh one per argument. The memo existed; its *scope* did not.
  Every dispatch re-simplified its subtree from scratch.
- This roadmap recorded that state as "implemented but not yet promoted to
  production path", `#[allow(dead_code)]`, and "mirrors the C++
  `gSimplifiedBoxProperty` behavior". All three were wrong, which is why the
  cost went unnoticed: the entry read as done.
- Fixing the scope took the corpus from 18.1 s to 10.7 s (3.81× → 2.30× vs
  C++ Faust) and `reverb_designer` from 7.2 s to 0.75 s, which is faster than
  the reference's 0.84 s.
- The lesson for the rules in §1: rule 2 asks for "an explicit key". A key is
  not enough — the *lifetime* of the table the key indexes is the other half,
  and it is the half that is easy to get wrong without any test noticing,
  because a too-short lifetime is merely slow and a too-long one is silently
  incorrect.

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
- it is intentionally kept separate from traversal/lowering memoization,
- since `29bf4df6` (2026-08-12) the type is still defined here but the mainline
  instance is a field of `compiler::BoxCompileOutput`: it is created in
  `pipeline_to_boxes`, filled by the arity phase, and consumed by the
  propagation phase of `pipeline_boxes_to_signals`. Arity and propagation always
  shared it; what changed is that it now crosses a public API boundary, so a
  host that calls `compile_*_to_boxes` may hold the filled table for an
  arbitrary time, or never continue (the `-e` expansion path drops it unused).
  Its keys stay valid because the arena travels in the same struct, which is the
  §1 test: table and keys expire together,
- `crates/compiler/src/diagnostic_enrichment.rs` allocates its own throwaway
  `ArityCache` for the arity queries of a mismatched sequential composition; it
  is not the pipeline's.

### 2.7 `propagate`: grouped-UI DAG visitation cache

Status: implemented

Location:

- `crates/propagate/src/ui_build.rs`

Cache:

- `UiCollector.visited: AHashMap<FlatBoxId, UiCollectSummary>`

Purpose:

- prevents duplicate traversal of shared flat-box subtrees during UI
  extraction,
- avoids ghost controls and duplicated UI ownership artifacts.

### 2.8 `propagate`: De Bruijn lifting and aperture memoization

Status: implemented

Location:

- `crates/propagate/src/engine.rs`

Cache:

- `PropagateMemo.liftn: AHashMap<(TreeId, i64), TreeId>`
- `PropagateMemo.aperture: AHashMap<TreeId, i64>`
- `PropagateMemo.slot_env_lift: AHashMap<(SlotEnvId, i64), SlotEnvId>`, added
  2026-08-08 (`c5d6bdb2`)

Purpose:

- avoids repeated full-subtree rewrites in recursive propagation,
- specifically targets the `liftn` and `aperture` hotspots observed in
  profiling on recursive/shared DAGs,
- `slot_env_lift` is the environment counterpart of the lifted tree C++
  synthesizes: repeated entry into the same recursion scope reuses one
  `SlotEnvId` (§2.16) instead of building an equal chain again, which is what
  keeps §2.15's key canonical.

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

### 2.9b `normalize`: table promotion cache

Status: implemented 2026-08-28 (`80e55328`)

Location:

- `crates/normalize/src/table_promote.rs`

Cache:

- `TablePromoter.cache: HashMap<SigId, Option<SigId>>`

Purpose:

- the Rust port of C++ `SignalTablePromotion`: every table read index and every
  writable table write index is clamped to `[0, size - 1]` unless the index
  interval in the supplied type map proves it already in range,
- the memo keeps the rewrite linear and, more importantly, keeps sharing: one
  access reached twice is rewritten once and both parents point at the same
  rewritten node.

The sentinel, and why it is the same one as §2.9:

- `None` means "visited and unchanged, return the node itself"; `Some(r)` means
  "rewritten to `r`". That identity value doubles as the cycle sentinel: a `Rec`
  node is entered into the table as `None` before its body is visited, so a back
  edge returns it unchanged,
- as in §2.9, the rebuilt `rec(new_body)` is deliberately not written back, so a
  second visit yields the original id. This is `sigMap` parity, not an
  oversight.

Constraints:

- the type map must be the annotation of the *current* forest: the pass runs as
  staging step 2.10b, right after a fresh `retype`, because a stale map silently
  mis-decides the clamp,
- the cache is forest-wide but one call long, and the pass runs once per
  prepare, gated by `-ct`.

Validation:

- eleven unit tests, of which `shared_access_is_rewritten_once` is the one that
  pins the memo,
- `crates/compiler/tests/check_table.rs` for the end-to-end behaviour. No
  performance measurement exists for this cache; it was written for sharing, not
  for speed.

### 2.10 `normalize`: promotion cache in normal-form pipeline

Status: implemented

Location:

- `crates/normalize/src/normalform.rs`, `SignalPromoter`

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

### 2.11 `sigtype` / `transform`: type results, deliberately short-lived

Status: implemented; rewritten 2026-09-11 because the entry described state that
was deleted on 2026-03-19 (`8b217616`, "Unify fast-lane promotion on canonical
sigtype"). `node_types`, `group_types` and `active_groups` no longer exist
anywhere in `crates/transform`.

Location:

- `crates/sigtype/src/rules.rs`, `TypeAnnotator`
- `crates/transform/src/signal_prepare/mod.rs`, `Staging`

Memoized state:

- `TypeAnnotator.env: HashMap<SigId, SigType>`, the memoized type results, with
  `in_progress: HashSet<SigId>` as the cycle guard for recursion groups,
- `Staging.sig_types: HashMap<SigId, SigType>`, the map the typed passes read.

The thing worth recording:

- `Staging::retype()` rebuilds `sig_types` **wholesale** at each of the five
  schedule points of the preparation (2.4, 2.6, 2.9, 2.12, 2.14, plus 2.10a for
  the table clamp), discarding the previous map each time,
- that is deliberate and is the opposite of a long-lived memo: every rewrite
  pass invalidates the typing, so the annotation is memoized *within* one sweep
  and thrown away between sweeps. A pass that read a stale map would clamp,
  promote or simplify against types that no longer describe its forest,
- this is still memoized analysis state rather than a lookup cache: the
  annotator seeds recursion groups and iterates them to a fixpoint.

### 2.12 `transform`: signal-to-FIR lowering cache, scoped by region

Status: implemented; scope stack since 2026-07-15 (`b6d589a6`)

Location:

- `crates/transform/src/signal_fir/module/region.rs` (the type),
  `module/core_lowering.rs` (the single lookup and the single insertion),
  `module/clocked.rs` (push and pop around a guarded body)

Cache:

- `RegionCache { scopes: Vec<HashMap<SigId, FirId>> }`, owned by
  `SignalToFirLower`, so one compiled module long; scope 0 is the current
  top-level sample loop and each open guarded region pushes one scope,
- `get_at(depth, sig)` walks from the effective depth towards the root;
  `insert_at` writes at the depth the region tracker reports, so an insertion
  redirected to an ancestor clock domain lands in the ancestor's scope and
  legitimately survives the child's close.

Purpose:

- memoizes already lowered FIR expressions for shared signal DAG nodes and keeps
  lowering linear in the shared graph size,
- the scope stack encodes the visibility rule: a value computed in a region may
  be reused in that region and its descendants only; any other reuse has to go
  through named storage.

The failure mode the isolation prevents:

- once shared sample-rate expressions are materialized as stack temporaries at
  their scheduled position, a flat cache lets two sibling `ondemand` blocks
  share a temporary that only the first block declares, and verification rejects
  the second block's load of an out-of-scope variable. A guarded child may only
  narrow reuse, never widen it.

Note on what stayed global:

- the first-emission trace (`emission_seen` / `emission_order`) is a separate,
  module-wide set, kept global on purpose so that narrowing lexical visibility
  does not lose the schedule-conformance trace compared against the chosen
  schedule.

Validation:

- `region.rs` unit tests `parent_entries_are_visible_in_children`,
  `child_entries_do_not_escape_to_siblings`,
  `redirected_parent_entries_survive_child_close`,
- `crates/compiler/tests/clocked_emission_structure.rs::sibling_ondemand_regions_do_not_reuse_local_scheduled_temporaries`,
  over all four scheduling strategies.

### 2.12b `transform`: FIR common-subexpression materialization, per scope

Status: implemented 2026-07-09 (`03127f30`)

Location:

- `crates/transform/src/signal_fir/cse.rs`

Caches:

- `count_fir_value_uses(...) -> HashMap<FirId, usize>`: how many times each
  shared FIR value is consumed **inside one scope**,
- `RewriteState.materialized: HashMap<FirId, (String, FirType)>`: the temporary
  already emitted for a shared value in the scope being rewritten.

Purpose, and the scope lesson:

- the pass used to run only on the flat tier buckets and deliberately did not
  descend into block or loop bodies. An `ondemand` guarded block is such a body,
  so a fully shared `O(N log N)` butterfly inside it was emitted as about `2N`
  inlined trees, `O(N^2.6)` arithmetic — worse than a naive DFT,
- `materialize_scope` now recurses into `If` / `Control` / loop / `Block` bodies
  as independent scopes: fresh counts, temporaries declared inside the body and
  never hoisted across the guard, with the name counters threaded so names stay
  unique across the scope tree.

Why the counts are recomputed per scope instead of shared:

- a use count is only meaningful for the scope that will emit the temporary;
  reusing a parent's counts would hoist a value across a guard that may not run.

Measurements (framed FFT):

- arithmetic operations at `N = 128`: 234 632 to 5 428, and the family back to
  `O(N log N)`,
- at `N = 256`: interpreter codegen 9.2 M lines to 60 k, compile 8.6 s to 1.8 s,
- numerics unchanged.

Relation to §3.4: occurrence counting already exists here, at FIR level. A
codegen-side cache would have to respect this same per-scope boundary.

### 2.12c `transform`: straight-line scalar table-load reuse

Status: implemented 2026-07-16 (`10c86026`, `28c29bd0`, `68604a30`, `edeb5c32`,
`8ed16955`)

Location:

- `crates/transform/src/signal_fir/cse.rs`, `reuse_straight_line_scalar_loads`

Cache:

- `cached_loads: HashMap<TableLocation, FirId>` with
  `TableLocation { name, access, index }` and
  `CanonicalTableIndex::{Constant(i32), Unknown}`,
- the payload is a load of the *first* stack temporary that already holds that
  table slot, not a re-emitted table read.

Lifetime:

- one flat scalar statement list, that is one sample-loop body; the pass refuses
  to run at all if the list contains a nested execution scope.

Invalidation, which is the whole content of this entry:

- a table store with a literal index invalidates only the slots that may alias
  it, and aliasing is assumed unless *proved* impossible,
- a store with a non-literal index, or an array shift, invalidates every slot of
  that table,
- a call, a tee, a DSP allocation or any nested control structure clears the
  cache entirely,
- a plain scalar store is deliberately **not** a table barrier: FIR names table
  storage explicitly, so committing a scalar cannot alias a table slot. Its
  value is still scanned for a nested call.

Why this is the correctness-sensitive one:

- every other cache in this document is an optimization whose failure is slow
  code. Here a missed alias loses an optimization, but a *false* proof of
  non-aliasing changes what a recursive DSP computes. Hence only literal
  subscripts are exact and two dynamic reads are never conflated.

Validation:

- seven unit tests in `cse.rs`, from `scalar_load_effects_distinguish_exact_and_dynamic_table_writes`
  to `straight_line_load_reuse_does_not_cross_nested_scope`,
- emitted-code witnesses in `crates/compiler/tests/signal_fir_lane.rs`,
- the C++ impulse matrix green for all 92 applicable DSPs under `-ss 0..3`; no
  compile-time change, this is a code-quality pass.

### 2.12d `transform`: FIR state names keyed by clock occurrence

Status: implemented 2026-07-18 (`ab41eada`)

Location:

- `crates/transform/src/signal_fir/module/mod.rs`:
  `state_name_by_node: HashMap<(SigId, Option<u32>), String>`,
  `scheduled_state_updates: HashSet<(SigId, Option<u32>)>`,
- `crates/transform/src/signal_fir/delay/manager.rs`, the delay lines of one
  occurrence.

What happened:

- delay lines, recursion carriers, current-sample bindings and update
  deduplication were keyed by `SigId` alone. Two sibling `ondemand` regions
  consume a shared hash-consed stateful payload, so the second region reused the
  state **and** the stack binding owned by the first, and FIR verification
  reported an undeclared recursion variable.
- The keys are now `(SigId, clock_context)`, with the equivalent contextual keys
  for recursion outputs, and generated names carry a deterministic domain
  suffix.

The rule this leaves behind:

- persistent state and local current-sample bindings may be shared only inside
  the same occurrence clock context. C++ gets this for free because its subgraph
  flow hands each region an independent state instance; the Rust prepared DAG
  keeps one shared node identity across sibling domains, so the key has to carry
  the domain.

Validation:

- focused delay-planner and all-strategy structural regressions, the
  `ondemand_18_toggle_morph.dsp` case compared frame by frame against the C++
  reference over 60 000 frames.

### 2.12e `transform`: delay planner dominance memo

Status: implemented; key extended 2026-07-18 (`ab41eada`)

Location:

- `crates/transform/src/signal_fir/delay/plan.rs`, `DelayPlanner`

Memoized state:

- `best_seen_delay: BTreeMap<(SigId, Option<u32>), i32>`, the largest
  path-accumulated delay a signal occurrence has been reached with,
- `scanned: BTreeSet<(SigId, Option<u32>)>`, the first-visit marker that records
  the per-occurrence carrier maximum.

Purpose and rule:

- one traversal produces both delay-plan maps; a node is revisited only when
  reached with a **strictly larger** accumulated delay, which is what fills the
  recursion-output sizing table without a second pass,
- the two maps are ordered, not hashed: their iteration order becomes struct
  field and clear-loop emission order, so a hash map would make generated code
  non-deterministic between runs.

### 2.13 `transform`: unary symbolic recursion discovery visitation set

Status: implemented

Location:

- `crates/transform/src/signal_prepare/rewrites.rs`

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

### 2.15 `propagate`: exact Box-to-Signal result memo

Status: implemented 2026-08-08; the slot-environment restriction removed
2026-08-30 (`b196ed7e`)

Location:

- `crates/propagate/src/result_memo.rs`
- `crates/propagate/src/engine.rs`

Cache:

- `PropagateMemo.results: PropagateResultMemo`, a compilation-scoped
  `AHashMap<PropagateResultKey, BusKey>`.

Key and payload:

- the exact key is `(FlatBoxId, SlotEnvId, UiPathId, PropagationModeKey,
  input bus)`;
- `PropagationModeKey` contains the clock environment/domain and FAD
  suppression state, so future eligibility expansion cannot alias those
  contexts accidentally;
- zero-, one-, and two-signal buses are stored inline; longer buses are
  canonicalized in a per-run `Arc<[SigId]>` interner;
- the payload is the exact output signal bus, using the same compact bus
  representation.

Purpose:

- adapts C++ `propagate(...)` / `gResult2Memo` to reuse an already propagated
  Box under the same canonical lexical, UI, execution-mode, and input context;
- removes the repeated recursive propagation exposed by smoothed
  Jiles-Atherton parameters while preserving canonical signal sharing and
  diagnostic origins.

Safety and scope:

- a linear whole-root scan enables replay only when the flat Box DAG contains
  neither forward/reverse AD nor `ondemand`/upsampling/downsampling wrappers;
- a non-empty pending-FAD-seed vector is an additional per-call barrier;
- nothing else is excluded. The table was limited to non-empty lexical slot
  environments between 2026-08-08 and 2026-08-30; that restriction is gone, and
  every eligible call past the warm-up now probes, as the C++ wrapper does;
- an exact-key hit records only its own provenance boundary, while the first
  miss records the full descendant derivation forest;
- the table is intentionally one propagation run wide. It must not cross
  arenas, compilation sessions, or mutable propagation contexts.

Adaptive policy and validation:

- the first 1,024 eligible calls run on the previous allocation-free path; only
  a traversal large enough to amortize hashing and retained input buses
  activates the table;
- the counters `result_memo_probes` and `result_memo_hits` in
  `crates/propagate/src/profile.rs` report the hit rate on stderr, but only if
  `FAUST_PROPAGATE_PROFILE` was set in the environment when the traversal was
  created and the run made at least a thousand calls. That is the instrument
  §1's rule 5 asks for, and it is the C++ flag of the same name;
- unit tests cover inline and interned buses, slot/UI key separation, warm-up,
  replay, and the AD/clock safety gate;
- on the 1,110-symbol faustlibraries corpus, the adaptive result is 71.25 s
  versus the 70.86 s pre-change reference (+0.55%), whereas always-on caching
  cost 79.17 s;
- retained generated C++ is byte-identical. The smoothed stereo sentinel drops
  from roughly 1.23 s to 0.215 s in propagation, and the two production
  Jiles-Atherton cases improve by 12.7x and 5.8x respectively.

Why the slot-environment restriction was wrong (2026-08-30):

- it barred the memo from the case it matters most for. A program written as
  top-level definitions evaluates to a DAG of shared, slot-free subtrees, and
  every reference re-propagated the whole subtree; C++ memoizes unconditionally
  and cuts each revisit at the first shared node,
- on `virtualAnalog.dsp`, then the worst case in `examples/` at 1.48× the C++
  compile time: propagation volume 7.5 M calls to 0.97 M (C++ about 8 600), the
  propagation stage 1.49 s to 0.89 s, total 2.78 s to 2.22 s, output
  byte-identical; over the whole `examples/` tree the ratio against C++ went
  from 0.81× to 0.74×,
- the 2026-08-08 concern, key bookkeeping on `parametric_eq`, no longer
  reproduces: the A/B is flat to slightly faster. What changed in between is
  that the keys became canonical (§2.16) and buses interned, which is the honest
  reading of the reversal — the original measurement was right about the
  representation it measured.

### 2.16 `propagate`: canonical context identities

Status: implemented 2026-08-08 (`c5d6bdb2`)

Location:

- `crates/propagate/src/context_id.rs`

Interners:

- `SlotEnv.interner: AHashMap<SlotBindingKey, SlotEnvId>`: slot bindings form a
  persistent chain, and equal construction histories receive equal compact ids;
  binding is a hash lookup, restoring a scope is one id assignment, and a
  shadowed binding stays in the parent chain,
- `UiPathContext.interner: AHashMap<Vec<UiGroupPathSegment>, UiPathId>`: one id
  per normalized grouped-UI path, interned once per propagation run.

Purpose:

- C++ passes hash-consed trees as the slot environment and the UI path, which is
  what makes its result-memo key pointer-sized. Rust cannot put a mutable map
  and an owned path in a key at that price; these ids adapt the representation,
- they are what made §2.15 affordable, and §2.8's `slot_env_lift` is built on
  them.

Constraint:

- the ids are one propagation run wide and must not cross arenas or sessions.

Validation:

- `slot_environment_interns_equal_binding_histories`,
  `slot_environment_shadowing_and_restoration_are_lexical`,
  `rebuilt_slot_chain_receives_the_same_identity`,
  `ui_paths_intern_equal_normalized_values_and_restore_exactly`.

### 2.17 `propagate`: forward-AD memos

Status: implemented 2026-07-10 (`e4181cc6`), extended 2026-09-05 (`adaca773`),
rescoped 2026-09-05 (`93f00111`)

Location:

- `crates/propagate/src/forward_ad.rs`, on `ForwardADTransform`

Caches, all one `fad(expr, seeds)` lowering long:

- `cache: AHashMap<SigId, Dual>`, the dual `{ primal, tangents }` of each
  rewritten node. It does two jobs: it keeps the rewrite linear on shared
  subgraphs, and it breaks cycles, since the recursion arm inserts a
  self-referential placeholder before descending so back edges resolve while the
  group is still being built,
- `dependency_memo: AHashMap<(SigId, usize, usize), bool>`: can the subtree
  rooted here depend on a seed? The tuple is the node, the number of binders the
  **transform** has entered, and the number this dependency walk has itself
  descended: seeds are spelled at the sum, reference levels are compared against
  the second alone. Keying on the total depth was tried and served an "internal"
  answer to a transform already inside the recursion, which broke every
  recursive FAD test,
- `aperture_memo: AHashMap<TreeId, i64>`: the de Bruijn aperture, same shape as
  §2.8 and §2.14 but a distinct instance with a narrower life,
- `od_aug_cache: AHashMap<SigId, SigId>`: the augmented twin of a clocked
  wrapper. This one is a sharing invariant rather than a speed memo: every
  consumer must route through the *same* augmented block, or a stateful body
  executes twice per fire.

The lifetime lesson (`93f00111`) — worth reading next to §2.5's:

- the dual cache was keyed by `SigId` alone, and entries were dropped on leaving
  a recursion body but carried **into** it. A de Bruijn term is relative to its
  binders, so one `SigId` means two different things across a binder: the very
  same node is the seed `prev_gain` outside and the back edge of an unrelated
  feedback body inside,
- the result was a wrong gradient, not slow code: an excitation that was itself
  a recursion (a noise generator, a sawtooth) received the seed's tangent, and
  the loss derivative picked up an extra term. It stayed hidden because every
  `optimizers.lib` loop clips its seed, which changes the node,
- the fix keeps, on entering a body, only the entries whose aperture is zero:
  closed terms mean the same thing at every depth, open terms do not and must
  not cross in either direction. The binder is the right boundary because it is
  exactly where the seed spelling changes, so scope and key validity expire
  together,
- a wider scope gives an inner recursion the seed's tangent; a narrower one
  (clearing at each binder) would be correct but lose the sharing.

Validation:

- `outer_seed_does_not_leak_into_an_inner_rec_body` and
  `fad_seed_not_poisoned_by_inner_rec_back_edge` pin the two directions at the
  arena level; `recursive_projection_seed_stays_a_leaf_inside_an_unrelated_recursion`
  compares the `fad` gradient with the hand-written one frame by frame and
  failed at frame 0 before the fix,
- `adaca773` is the payoff of `dependency_memo`: a recursion no seed reaches is
  no longer augmented. On the diode-clipper Newton example, 13 391 interpreter
  instructions to 4 643, and single-precision convergence restored, where
  augmenting the whole loop gave `inf * 0`.

### 2.18 `propagate`: stateful-RAD linearity classifier memo

Status: implemented 2026-04-28 (predates this document's first sweep)

Location:

- `crates/propagate/src/stateful_rad.rs`, `LinearityAnalyzer`

Cache:

- `memo: AHashMap<(TreeId, i64), ExprClass>`, the node and the active de Bruijn
  level, giving the triple "depends on the current recursion state", its
  linearity class, and its independent variation.

Purpose and constraint:

- keeps classification linear in the visited DAG for a fixed level,
- the level belongs in the key for the same reason as in §2.17: a node is
  independent in one nested recursion and state-dependent in another, and a
  nested body is classified one level up.

Validation:

- nine unit tests in the file, from `classifier_accepts_constant_coefficient_linear_recursion_as_lti`
  to `classifier_reports_rad_mode_for_de_bruijn_group_and_projection`. No
  published measurement.

### 2.19 `transform`: block-reverse-AD tapes and forward outputs

Status: `grad_cache` predates June; forward-output keys 2026-09-06 (`25a12e99`);
`tape_by_value` 2026-09-10 (`d5da77bd`)

Location:

- `crates/transform/src/signal_fir/module/bra.rs`, `module/build.rs`,
  `module/core_lowering.rs`; all one compiled module long

Caches:

- `grad_cache: HashMap<(SigId, usize), FirId>`: the gradient of one seed of one
  reverse-AD carrier, so a single backward sweep serves every gradient
  projection of the group; the companion `scheduled` set is the re-entry guard
  and this is its value side,
- `forward_output_keys: Option<HashMap<String, usize>>`: the output lane that
  carries a given forward value, built lazily on the first identity miss and
  keyed by the **shared-structure** dump of the signal,
- `tape_by_value: HashMap<FirId, (String, FirType)>`: the tape field already
  allocated for a lowered forward value, checked after lowering, so a second
  signal that lowers to the same value is aliased onto the existing tape instead
  of declaring a second tape array. The older `tape_store_var`, keyed by
  `SigId`, cannot see that: the canonical case is a recursion slot read inside
  its body and outside it, two signal ids for one value.

Why the key function mattered more than the hit rate:

- the forward-output map used to be keyed by the *readable* dump, which prints a
  shared graph as a tree and is therefore exponential in depth. On a six-line
  FDN calibrated against a room impulse response, 40 seeds, one block over the
  whole response, that key cost **5 minutes and 10 GB before a single
  instruction was lowered**; with the structure-preserving dump, 1 second and
  170 MB,
- this is the counter-example to §6's story: what was wrong was neither the
  presence of a cache nor its scope, but the cost of computing its key.

Measurements for `tape_by_value`, jointly with the trivially-evaluable rule
added in the same commit: one biquad section 21 tapes to 5; a 279-parameter FDN
4449 to 1391 tapes and 3.2 s to 1.0 s per 65 536-frame epoch.

Validation:

- `crates/compiler/tests/bra_tapes.rs`, which counts tapes —
  `a_value_read_inside_and_outside_its_recursion_gets_one_tape` is the
  `tape_by_value` regression — plus `bra_tape_size.rs` for the sizing and
  `rad_runtime.rs` for gradients against finite differences.

### 2.20 `transform`: clock-environment inference sweep

Status: implemented 2026-07-07 (`309ba3d7`)

Location:

- `crates/transform/src/clk_env/mod.rs`, `Inference`

Memoized state:

- `cache: AHashMap<SigId, ClkEnv>`, the domain in which a signal is computed.

Lifetime, which is the point:

- one sweep, not one compilation: the fixpoint builds a fresh `Inference` for
  every group in every Kleene round, seeded from the *previous* hypothesis, so
  the iteration is order-independent and deterministic. The final sweep's table
  is then published whole as the annotation map,
- a value is only valid relative to the group hypothesis held in that same
  sweep, which is exactly why it cannot cross rounds: between rounds the
  hypothesis changes and every entry with it.

Note:

- like §2.11, memoized analysis state rather than a lookup cache: two entries
  are written by the pass itself, the hypothesis seed for a recursion group so
  back edges resolve, and the inner domain for a wrapper's clock child.

Validation:

- twelve tests in `clk_env/tests.rs`, one per inference rule;
  `r_proj_fixpoint_pulls_group_into_clocked_domain` and
  `fixpoint_least_solution_keeps_untouched_group_at_audio_rate` are the two that
  a cache surviving across rounds would break, by returning a solution that is
  not the least one.

### 2.21 `transform`: `Special` scheduling sequence summaries

Status: implemented 2026-07-12 (`14376ddd`), finalized 2026-07-14 (`e7edbf08`)

Location:

- `crates/transform/src/schedule/special.rs`

Cache:

- `memo: AHashMap<D::Node, SequenceSummary<D::Node>>`, one per `compact(...)`
  call; the payload is `{ len, last }`, the logical length of the sequence a
  node would produce and the last position of every node in it.

Purpose:

- C++ `spschedule` materializes a duplicate list whose length follows the DAG's
  *path count*, then keeps each node's last occurrence. Memoizing the summary
  computes the same positions without ever building the sequence, so the result
  is the identical order at a cost linear in the shared DAG.

Why it is a change of complexity class, not a constant factor:

- for a two-wide ladder the logical sequence is `2^(layers + 1) - 2`: 20 nodes
  give 2046 entries, 40 nodes about two million, 60 nodes about two billion,
- the guardrail test schedules a 160-node ladder, whose logical sequence has
  about `2^81` entries, in under a second.

Constraints:

- positions are `u128`; if even the logical sequence would overflow that, the
  scheduler falls back to deterministic depth-first rather than wrapping,
- the literal C++ algorithm is kept under `cfg(test)` as an executable oracle,
  and equality with it is checked for one to ten layers.

Validation:

- `crates/transform/src/schedule/tests/growth.rs`, three tests.

## 3. Planned Additions

The items below are ordered by expected leverage and safety.

### 3.1 `propagate`: result-memo eligibility expansion

Status: deferred; the original result memo is implemented in §2.15.

The 2026-08-06 experiment used an expensive mutable-environment and owned-bus
key. It was slower on `virtualAnalogForBrowser.dsp` (10.6 s to 13.9 s, 12% hit
rate) despite byte-identical output. That finding rejected the representation,
not exact result replay. Canonical slot/UI identities and compact buses enabled
the current implementation.

The remaining work is to replace the conservative whole-root exclusion with a
per-subtree eligibility fact, but only after AD seed accumulation and
clock-domain state deltas have an explicit replay protocol. Until then, do not
widen §2.15's gate.

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

Status: planned in `codegen`; already implemented one layer earlier, in the FIR
common-subexpression pass (§2.12b), where counts are recomputed **per execution
scope** on purpose. Any codegen-side table has to respect that boundary, so what
remains planned is narrower than this entry was written for.

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

Status: planned in `codegen`; the accumulated-delay side already exists in
`transform` (§2.12e), as a dominance memo keyed by signal occurrence rather than
a plain `SigId → usize`, which is a warning for this entry: the "precise delay
notion" the constraint below asks for turned out to include the clock
occurrence.

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

- `crates/propagate/src/engine.rs`, `flatten_route_ints`

Likely cache shape:

- `AHashMap<TreeId, Vec<i64>>`

Why:

- `flatten_route_ints` is pure and easy to cache.

Constraint:

- lower expected payoff than the items above,
- only worth adding if profiling shows repeated route decoding.

### 3.7 `eval`: hash-consed environment layers

Status: planned, and the most structural item on this list

Target:

- `crates/eval/src/environment.rs`

Why:

- C++ hash-conses its environments, so its `(tree, env)` property table hits
  across different calls that rebuilt the same bindings. Ours allocates a fresh
  layer per binding, so §2.4b's key is fresh whenever an equal environment is
  rebuilt — which is what made a list traversal cubic and forced the workaround
  in §2.4c,
- `propagate` already did exactly this for its own contexts (§2.16), and it is
  what turned §2.15 from a pessimization into a win. The same construction
  applied to `EnvLayer` would let §2.4c's fast path be an optimization rather
  than a correctness-sensitive exception list.

Constraint:

- layers hold `EvalValue`s, not only tree ids, so interning needs a canonical
  identity for closures (§2.4d supplies one) and for pattern matchers, which are
  still host-side mutable values and are the reason §2.4b does not cache them,
- the barrier flag and the parent link are part of the identity.

Validation:

- the corpus expansions and the tutorial's learning programs, which are what
  caught the two mistakes in §2.4c,
- a repeat of the `ba.take` measurement: the fast path should become
  unnecessary, not merely redundant.

## 4. Explicit Non-Goals

These are not good general-purpose memoization candidates unless profiling and
semantics clearly justify them:

- `eval` deep reduction with an implicit `(Tree, Environment)` cache key,
- fully generic `propagate_inner` caching across arbitrary input/context state,
- tiny tag-decoding helpers where the cost is dominated by larger traversals,
- caches that silently merge results from different precision, typing, or
  backend modes.

Two structures that look like entries for §2 and are not:

- `VectorValueCache` (`crates/transform/src/signal_fir/vector/route/session.rs`)
  is a routing environment, not a memo: its control, owned and inline maps are
  written once, a second definition of a key is a hard error, and a miss is
  either an error or a request to lower inline — recomputation is never the
  alternative, so rule 1 does not apply. Its `transport_loads` field is the one
  real memo inside it, one load node per transport per session,
- the FFI factory cache (`crates/ffi-common/src/factory_cache.rs`) memoizes
  whole compilations keyed by a backend identity string, but it is process-wide,
  outlives every arena, and is driven by host calls; its discipline is reference
  counting and pointer invalidation, with its own contract document. It is worth
  one line here only for the hazard it shares with the list above: the Cranelift
  key has to carry the optimization level, the canonicalized argv, the foreign
  functions and the semantic FIR fingerprint, or two different compilations
  coalesce. The interpreter side had that hazard realized — it keyed by the
  `.fbc` header's field, which the codegen layer writes empty, so the second
  factory created through the FFI was dropped and its caller handed the first.
  Fixed on 2026-09-11: the FFI computes the key libfaust computes, from the
  source and its normalized options, or from the digest of the bitcode text.

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

1. ~~`eval`: give the existing `box_simplification` memo a compilation-scoped
   lifetime (§2.5).~~ **Done 2026-08-06**; worth 7.4 s of the corpus's 18.1 s.
2. ~~`propagate`: cache only provably context-free closed subtree propagation
   (§3.1).~~ **Attempted and rejected 2026-08-06**: slower, 12 % hit rate.
3. `normalize`: introduce a signal normal-form cache.
4. `codegen`: add occurrence counting cache once the scheduling path is stable.

### What the day's measurements actually changed

The reordering above was the point when it was written, and it was still not
enough. Items 2–4 had been listed first for two years on plausibility; item 2
has since been implemented and measured as a *pessimization*. Three plausible
memoizations were tried on 2026-08-06 and all three lost:

| change | result |
|---|---|
| `box_simplification` scope fix (§2.5) | **3.81× → 2.30×** — the one win |
| `liftn` closed-subterm fast path | no change (14.19 s → 14.49 s) |
| `propagate_in_slot_env` result memo (§3.1) | slower (10.6 s → 13.9 s) |
| `SmallVec` for propagation results | slower (10.7 s → 15.3 s) |

What actually moved the remaining cost was **not memoization at all**: a
combined-DFA lexer (2.13× → 1.21×) and swapping the platform allocator
(1.21× → 0.82×). The corpus now compiles *faster* than C++ Faust.

The standing lesson for items 3 and 4: this roadmap's §1 rules test whether a
computation *could* be memoized, never whether the repeat rate justifies it.
Measure the hit rate on a case where the stage dominates before writing the
cache — and pick that case deliberately, because the impulse corpus put
propagation at 2.2 % where a real DSP puts it at 82 %.

### What happened after that day (swept 2026-09-11)

The table above ends on 2026-08-06, and the entry it marks as a pessimization —
exact result replay in propagation — has since been reversed, not by revisiting
the decision but by changing the representation the key is built from:

| change | result |
|---|---|
| canonical slot/UI identities (§2.16) | the key that made §2.15 affordable |
| exact propagation result memo (§2.15) | the smoothed cases 12.7× and 5.8× |
| dropping its slot-environment gate (§2.15) | 7.5 M calls to 0.97 M |
| closure interning (§2.4d) | 975 k calls to 53 k, 2.22 s to 1.54 s |
| normal-form fast path (§2.4c) | a list of 1090 elements: hours to 3.1 s |
| CSE per execution scope (§2.12b) | the framed FFT back to `O(N log N)` |
| structure-keyed forward outputs (§2.19) | 5 min and 10 GB to 1 s and 170 MB |

Four of those seven are not new caches at all: three are a better key or a
canonical identity, one is a better scope. That is the shape the evidence keeps
taking, and it is why rules 6 and 7 now sit next to rule 5.

Current order, revised 2026-09-11:

1. `eval`: hash-consed environment layers (§3.7) — the last place where the
   port still pays a cost C++ does not, and the fix that would retire §2.4c's
   exception list.
2. `normalize`: a signal normal-form cache (§3.2), unchanged from 2026-08-06.
3. `codegen`: occurrence counting (§3.4), now only the part §2.12b does not
   already cover.

Nothing on this list should be written before its hit rate is measured on a
case where the stage dominates; `FAUST_PROPAGATE_PROFILE` is the model for the
instrument, and every future cache should ship one.
