# Porting analysis — C++ memoization optimizations (commits cb6891ae + 2aec3bff0 + 0ef3eb314)

**Date**: 2026-07-04 (updated 2026-07-05 with commit `688c0a478`)
**Status**: Analysis complete, implementation plan proposed
**Upstream range**: `cb6891ae~1..HEAD` in `/Users/letz/faust` — initially 3 commits (2026-07-03,
Yann Orlarey); extended on 2026-07-05 with `688c0a478` (tlib speedup, §1.3). The remaining new
commits (`f607b2780`, `d80e2271b`, `29cda8807`) are Windows release/packaging fixes plus a
Windows `Garbageable` cleanup follow-up to §1.3 — all N/A for faust-rs.

---

## 1. The upstream commits

### 1.0 `cb6891ae` — "make CTree/Symbol hash tables grow instead of fixed-size"

The two hand-rolled intrusive chained hash tables at the heart of tlib —
`CTree::gHashTable` (hash-consing of all trees) and `Symbol::gSymbolTable`
(name interning) — were fixed-size C arrays (400009 and 511 buckets). Small
files paid a huge mostly-empty table; large files degenerated into long chain
scans on every `CTree::make()` (the hot path: most calls are cache hits on
already-built subexpressions).

The commit makes both tables start small (511 buckets) and rehash-double
(next prime ≥ 2×) once the load factor crosses `gGlobal->gHashLoadFactor`
(0.7 by default). Implementation details worth noting:

- Node addresses are stable across resize — only the intrusive `fNext`
  chaining is rewired, so every `Tree`/`Sym` pointer held elsewhere stays
  valid (a constraint faust-rs does not have, since `TreeId` is an index, not
  an address).
- The load-factor check runs only on *colliding inserts*, never on lookups or
  collision-free inserts.
- Lazy first allocation guards against static-initialization-order issues.
- A new CLI option `-hlf/--hash-load-factor <n>` exposes the threshold as a
  pure tuning knob (documented as never changing generated code); 0.7 was
  chosen empirically as keeping nearly all of the time win at roughly neutral
  memory.

### 1.1 `2aec3bff0` — "tlib+propagate: lazy per-node properties, fast-path slot, tuple memoization"

Four independent changes:

**(a) Propagation memoization re-keyed on plain C++ data.**
The old memo built a hash-consed key tree
`tree(PROPAGATEPROPERTY, slotenv, path, box, listConvert(lsig))` on *every*
`propagate()` call, including cache hits. On large parallel structures
(FFT512 measured `avg_in ≈ 684` inputs per `boxPar` call) constructing that
temporary cons-list dominated the whole propagation phase. The fix stores the
same logical key as a plain struct in a `std::unordered_map`:

```cpp
struct PropagateMemoKey {
    Tree    fSlotEnv;   // hash-consed → pointer identity is canonical equality
    Tree    fPath;      // UI path
    Tree    fBox;
    siglist fInputs;    // vector of hash-consed signal pointers
    bool operator==(...)  // full field + per-input comparison
};
std::unordered_map<PropagateMemoKey, siglist, PropagateMemoKeyHash>
```

Correctness comes from exact `operator==`; the hash only distributes buckets.

**(b) Cache lifetime scoping.** `PropagateMemoScope` (depth-counted RAII)
clears the memo at the *outermost* `boxPropagateSig` boundary, so entries never
survive into another libfaust compilation, and `Map().swap(fMap)` releases the
bucket array too.

**(c) Lazy per-node property lists.** `CTree::fProperties` becomes
`plist*` (nullptr until first property) — measured ~72 % of `CTree` never get
a property. Plus a single-slot `fFastProperty` field claimed by the hottest
property (~20 % of all property traffic).

**(d) Opt-in propagation profiler.** With `-time` +
`FAUST_PROPAGATE_PROFILE` set, per-box-kind counters (calls, hits, misses,
inclusive time, `realPropagate` time, average input arity) are printed. This
is what produced the FFT512 diagnosis in the first place.

### 1.2 `0ef3eb314` — "Introduction of a binary memoization primitive: property2\<Tree\>"

`eval(box, env)` was memoized as
`setProperty(box, tree(EVALPROPERTY, env), value)`: every distinct `env` mints
a fresh hash-consed compound key tree *and* piles a new entry onto the same
box's property list (one real case reached **56 000+ entries on a single
node**, making its `std::map` lookups the bottleneck; with a flat linear-scan
buffer the compile went quadratic). `property2<Tree>` replaces this with a
two-level side table owned by `global` (correct lifetime for libfaust
multi-compilation):

```cpp
std::unordered_map<Tree /*a*/, Entry> fOuter;   // pointer-keyed, no key tree minted
struct Entry { Tree fB; Tree fValue; std::map<Tree,Tree>* fInner; };
// first (b, value) pair inline; promoted to a nested map on the 2nd distinct b
```

Both `EvalProperty` and the pattern-matcher `PMProperty` move onto it.

### 1.3 `688c0a478` — "tlib: faster version of the Tree library (array-based make, non-virtual Garbageable)" (2026-07-04)

Three independent axes, continuing the same campaign:

**(a) Array-based `make` — no temporary container on the interning hot path.**
The fixed-arity constructors `tree(n, a)` … `tree(n, a, b, c, d, e)` used to
materialize a `tvec` (heap `std::vector`) on *every* call, including cache
hits (the overwhelming majority in a compiler that constantly re-references
already-built subexpressions). They now build a stack array and call new
array-based overloads — `CTree::make(n, ar, const Tree br[])`,
`calcTreeHash(n, ar, br)`, `equiv(n, ar, br)`, `calcTreeAperture(n, ar, br)` —
so hash, lookup, and comparison all run off the stack array; the owned
`fBranch` vector is populated (`assign`) only when the tree is actually
created.

**(b) Non-virtual `Garbageable`.** `CTree : public Garbageable` replaces
`public virtual Garbageable`, removing the virtual-base overhead from every
node; the allocation registries move out of the `global` singleton into
tlib itself with construct-on-first-use statics (`29cda8807` is a Windows
follow-up fix to this cleanup path).

**(c) tlib becomes a standalone library.** tlib no longer includes
`global.hh`/`exception.hh`: new `tlib-error.{hh,cpp}`, `export.hh`
(`TLIB_API`), `tlib.{hh,cpp}` session init/cleanup; the `-hlf` knob now goes
through `CTree::setHashLoadFactor()` (the CLI option survives and calls the
API); `nil`/`cons` symbols are lazily owned by the library; non-tlib files
move out (`num.hh` → normalize, `shlysis` → signals, `compatibility` → utils).

---

## 2. Mapping onto faust-rs — the key observation

**These C++ commits converge on the design faust-rs already has.** faust-rs has
no per-node property lists at all: `TreeArena` nodes carry only
`(NodeKind, children)`, and every memo is a side `AHashMap` keyed by plain data
(`TreeId` is a content-addressed `u32`). The three C++ pathologies these
commits fix — compound key trees minted per lookup, property entries piling up
on one node, property storage bloating every node — structurally cannot occur
in faust-rs.

Per-item status:

| Upstream change | faust-rs status |
|---|---|
| (0) growable CTree/Symbol hash tables | **N/A by construction.** `TreeArena`'s interners (`interner0/1/2/n`, [arena.rs:226](../crates/tlib/src/arena.rs)) and `SymbolTable::to_id` are `AHashMap`s — hashbrown SwissTables that already start small and resize by load factor automatically. The C++ commit essentially brings the hand-rolled tables up to what the Rust standard maps do by default. See §2.1 for the two residual items (`-hlf` CLI parity, capacity hints). |
| (1.3a) array-based `make` (no temp container on intern lookups) | **Mostly N/A — one real residual.** faust-rs arities 0/1/2 already intern through tuple-keyed maps with zero allocation on hit *or* miss (better than upstream, which still `assign`s `fBranch` on creation). But the arity-≥3 path builds an `Arc<[TreeId]>` key **on every call, including cache hits** ([arena.rs:367](../crates/tlib/src/arena.rs)) — the same pathology upstream just removed. See §2.1 item 3. |
| (1.3b) non-virtual `Garbageable` | **N/A by construction.** No inheritance, no vtables, no GC registries: nodes are plain structs owned by the arena `Vec`, freed by `Drop`. |
| (1.3c) tlib as a standalone library | **Already achieved.** `crates/tlib` has no dependency on compiler globals, owns its own error types, and pre-interns `nil` at `TreeArena::new()`. Upstream is converging on the crate boundary faust-rs started with. |
| (1a) propagate memo on plain-struct key | **MISSING — the main portable item.** faust-rs has *no* memoization of propagation results at all (see §3). The new C++ key design is exactly the shape a Rust port should take. |
| (1b) memo lifetime scoping | **Already equivalent.** `PropagateMemo` is created per top-level call in `propagate_typed_with_ui_options` ([api.rs:49](../crates/propagate/src/api.rs)) — same semantics as `PropagateMemoScope` clearing at the outermost boundary. Keep it per-call (see §4.4). |
| (1c) lazy property lists + fast slot | **N/A by construction.** No `fProperties` exists; side tables are already lazy and pay-per-use. Nothing to port. |
| (1d) propagation profiler | **Portable, recommended.** faust-rs has no per-box-kind propagation profile; porting it first gives the measurement needed to validate the memo (see §4.3). |
| (2) `property2` for `eval(box, env)` | **Already implemented.** `LoopDetector::eval_cache: AHashMap<EvalCacheKey, EvalValue>` with `EvalCacheKey { expr: TreeId, env_key: EnvFrameKey }` ([environment.rs:108](../crates/eval/src/environment.rs)) is a flat plain-data-keyed side map — the same fix, in arguably simpler form (no two-level nesting needed because no per-node piling exists). |
| (2) `property2` for the PM property | **Already implemented differently.** `LoopDetector::automaton_cache` keys by the `TreeId` of the *evaluated* rule list, which already folds in environment effects. No compound key trees, no piling. |

### 2.1 Residual items from commit cb6891ae (hash-table growth)

Although the mechanism is N/A, three small follow-ups are worth considering:

1. **CLI parity for `-hlf/--hash-load-factor <n>`.** The faust-rs CLI
   normalizes short C++-style flags in
   [args.rs](../crates/compiler/src/cli/args.rs) and rejects unknown ones.
   Scripts and harnesses that pass identical flag sets to both compilers
   (e.g. benchmarking wrappers exploring `-hlf` values) would fail on
   faust-rs. Since the option is documented upstream as "never changes
   generated code", accept `-hlf <n>` and ignore the value (hashbrown's load
   factor is fixed at ~87.5 % and not worth emulating). Optional, one small
   parser arm.
2. **Capacity hints are the Rust-side analogue of the tuning knob.**
   `TreeArena::with_capacity` / `with_capacities` already exist
   ([arena.rs:254](../crates/tlib/src/arena.rs)) but `TreeArena::new()` starts
   everything at 0. If the P0 profiler (§4) shows rehash time in interner
   growth on large files, seed the compiler's arena with a modest capacity
   (e.g. 64 K nodes) — the same trade-off `-hlf` explores, expressed the Rust
   way. Data-driven, not speculative.
3. **Allocation-free arity-≥3 intern lookups** (from `688c0a478`, §1.3a).
   `TreeArena::intern`'s `_ =>` arm does `Arc::from(children)` *before* the
   `interner_n.get(&key)` probe, so every arity-≥3 intern — hit or miss —
   pays a heap allocation plus refcount traffic, exactly the temporary-
   container cost upstream just removed from `CTree::make`. Rust fix shape:
   probe with a borrowed key first and only materialize the `Arc` on insert —
   either via `hashbrown`'s raw-entry API (hash the `(kind, &[TreeId])` pair
   manually, compare with a slice-aware equality closure) or an
   `Equivalent`-style borrowed key type. Cons cells (the truly hot family)
   are arity 2 and already allocation-free, so the win is limited to wide
   nodes (`route`, slider parameter lists, waveform/table shapes) — measure
   with the P0 profiler before and after on a UI-heavy and an FFT-like file.
   Small, self-contained, byte-identical by construction.

### 2.2 What the port reduces to

So the port reduces to: **finally add propagation-result memoization to
`crates/propagate`** — a gap identified in
[propagation-performance-analysis-plan-2026-03-24-en.md](propagation-performance-analysis-plan-2026-03-24-en.md)
(Phase 1a, "implementation deferred", ~5× slowdown vs C++ on
`clarinetMIDI.dsp`, propagation = ~850 ms of 880 ms total) — using the
upstream-validated key design, plus the profiler to prove it.

The March 2026 plan's other phases have since landed independently
(`2b384963` single-allocate intern path ≈ Phase 3a; `97fc38a6` `SymId` interning
≈ part of Phase 2a), but Phase 1a/1b (the big ones) were never merged; an
untracked draft exists as `0001-perf-propagate-memoize-propagate_in_slot_env-liftn-a.patch`
against the pre-split `propagate/src/lib.rs` layout and no longer applies.

---

## 3. Why the Rust key is harder than the C++ key

C++ `propagate(slotenv, path, box, lsig)` is a pure function of four values,
all hash-consed trees (or vectors of them) → pointer identity keys work
directly. The Rust equivalent `propagate_in_slot_env(arena, box_tree, inputs, ctx)`
threads a richer mutable context ([engine.rs:1214](../crates/propagate/src/engine.rs)),
and each field must be classified:

| `PropagateContext` field | Role vs the C++ key | Treatment in the memo |
|---|---|---|
| `slot_env: &mut AHashMap<BoxId, SigId>` | = C++ `slotenv`, but a *mutable map*, not a persistent hash-consed list. Mutated in place at `Symbolic` boundaries ([engine.rs:486](../crates/propagate/src/engine.rs)) with save/restore. | Must be part of the key. Options in §4.1. |
| `current_groups: Vec<UiGroupPathSegment>` | = C++ `path`. UI leaves resolve `control_ids[(box, group_path_hash(current_groups))]` ([engine.rs:321](../crates/propagate/src/engine.rs)), so results genuinely depend on it. | Include `group_path_hash(&ctx.current_groups)` (already computed at UI leaves; it is a stable u64 of the group path). |
| `clock_env: TreeId` | **Hard correctness requirement, not just future-proofing** (P0.3 of the ondemand roadmap, folded here per [ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md](ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md) §2.3). In C++ the propagate memo key *includes* the clockenv ([propagate.cpp:918-929]); omitting it would return the same signal for the same box propagated under two different domains — recreating the very bug P0.3 audits. Since P0.2 landed, `clock_env` is `nil` at the top rate or a per-instance-unique `SIGCLOCKENV` token, so key inclusion is sufficient *and* cheap. | Include the `TreeId` in the key — **mandatory**. Validation §5 item 9 pins it. |
| `suppress_fad: bool` + `pending_fad_seeds: Vec<SigId>` | **No C++ analogue — this is a side channel.** Under `suppress_fad`, propagating a `ForwardAD` box *appends* seeds to `pending_fad_seeds` and returns primal-only outputs. Replaying a cached result would silently drop seeds. | Do **not** put in the key; instead **bypass the memo** when `ctx.suppress_fad` is true *or* `contains_forward_ad(arena, box_tree)?` holds. This mirrors the FAD-arity carve-outs already present in `propagate_in_slot_env`'s output checks. |
| `cache: &mut ArityCache`, `control_ids` | Pure analysis caches / read-only per-traversal tables. | Not in key; their per-top-level-call stability is exactly why the memo must also stay per-top-level-call (§4.4). |
| `memo: &mut PropagateMemo` | Carrier — the new table lives here next to `liftn` / `aperture`. | — |

One more Rust-only subtlety: `propagate_inner` is *fallible*
(`Result<Vec<SigId>, PropagateError>`). Only `Ok` results are cached; errors
propagate uncached (they abort compilation anyway).

---

## 4. Porting plan

### Phase P0 — profiler first (port of upstream (1d))

Small, zero-risk, and it decides whether P1's complexity is warranted per key
variant. In `crates/propagate/src/engine.rs` (or a new `profile.rs`):

- `enum PropKind { Atom, WireCutSlot, Prim, Ui, Group, Seq, Par, Split, Merge, Rec, Route, Symbolic, Fad, Rad, Other }` — note the two AD kinds C++ doesn't have; classification is a cheap match on `FlatNodeKind`, so no C++-style `isBox*` cascade is needed.
- Per-kind `{ calls, hits, misses, input_signals, total: Duration, real: Duration }`, stored in `PropagateMemo` (naturally per-traversal, no globals).
- Gate on `std::env::var_os("FAUST_PROPAGATE_PROFILE")` checked **once** at `PropagateMemo` construction (upstream uses a function-local static; a field set at construction is the Rust idiom and keeps one traversal self-consistent).
- Print to stderr at the end of `propagate_typed_with_ui_options` when ≥ 1000 calls, same tab-separated columns as C++ (`kind calls hits misses total_s real_s overhead_s avg_in`) so outputs can be compared side by side against `faust -time` + `FAUST_PROPAGATE_PROFILE`.

Deliverable: profile tables for `clarinetMIDI.dsp`, an FFT-like case (the C++
motivating workload), and 2–3 FAD-heavy files from the repo corpus
(`fad_fdn_rev.dsp`, `fad_biquad_spectral_v3.dsp`).

### Phase P1 — propagation-result memo (port of upstream (1a))

Add to `PropagateMemo`:

```rust
pub(crate) struct PropagateResultKey {
    box_tree: FlatBoxId,
    inputs: SmallVec<[SigId; 4]>,   // SigId is a small Copy id — cheap to hash/compare
    slot_env: SlotEnvKey,           // see below
    group_path: u64,                // group_path_hash(&ctx.current_groups)
    clock_env: TreeId,
}
prop: AHashMap<PropagateResultKey, Vec<SigId>>,
```

Wrap the *body* of `propagate_in_slot_env` (after the input-arity check, which
must still run and error identically):

```rust
let memoizable = !ctx.suppress_fad && !contains_forward_ad(arena, box_tree)?;
let key = memoizable.then(|| make_key(...));
if let Some(k) = &key {
    if let Some(hit) = ctx.memo.prop.get(k) { return Ok(hit.clone()); }
}
let outputs = /* existing body incl. output-arity checks */;
if let Some(k) = key { ctx.memo.prop.insert(k, outputs.clone()); }
Ok(outputs)
```

`contains_forward_ad` already exists and is used on the same path; if
profiling shows it hot, memoize it per `FlatBoxId` in `ArityCache` (it is a
pure structural predicate).

**`SlotEnvKey` — the one real design decision.** C++ gets slot-env identity
for free (persistent cons-list, pointer key). Rust's `SlotEnv` is a mutable
`AHashMap<BoxId, SigId>`. Two stages:

- **P1a (recommended first): memoize only when `ctx.slot_env.is_empty()`**
  (`SlotEnvKey` = unit, key omits it). After eval/a2sb lowering, the vast
  majority of propagation runs outside any `Symbolic` scope; the March 2026
  draft patch took the same cut. Zero collision risk, tiny diff, and the P0
  profiler will show exactly how many calls are excluded (`Symbolic`-kind
  descendants).
- **P1b (only if the profiler shows real traffic under non-empty slot envs):**
  give `SlotEnv` a *generation id* — replace the raw `AHashMap` with a small
  wrapper that increments a `u64` epoch on every `insert`/`remove` **and
  interns the sorted binding vector** in a `AHashMap<Vec<(BoxId, SigId)>, SlotEnvId>`
  the first time each epoch is keyed. `SlotEnvKey = SlotEnvId` then has C++
  pointer-identity semantics: equal ids ⇒ equal envs, and *equal envs reached
  by different mutation paths still unify* (better than an epoch alone, which
  would miss the very common bind→unbind→re-bind-same pattern in `par(i, N, …)`
  bodies). Interning cost is paid once per distinct env content, like C++
  hash-consing of the slotenv list — but only for envs that actually get keyed.

**What P1 must NOT do:** hoist the memo across top-level `propagate_typed_*`
calls. `control_ids` and the grouped-UI build are per-call; `group_path` hashes
are only meaningful against that call's registry. Upstream (1b) reaches the
same conclusion from the libfaust side — its `PropagateMemoScope` clears at
exactly this boundary. faust-rs's per-call `PropagateMemo` lifetime is already
correct; document it, don't change it.

### Phase P2 — `liftn` aperture fast-path (companion, from the March plan)

Not part of the upstream commits (C++ `liftn` has had the `aperture == 0`
guard for years) but it attacks the same hot loop and the current Rust `liftn`
([engine.rs:1264](../crates/propagate/src/engine.rs)) still recurses into
closed subterms:

```rust
// at the top of liftn, after the memo probe:
if de_bruijn_aperture_with_memo(arena, root, &mut memo.aperture) < threshold {
    return root;   // no reference at or above threshold can exist below root
}
```

`de_bruijn_aperture_with_memo` already exists in `tlib` and shares
`PropagateMemo::aperture`, so this is a two-line change. Note the `< threshold`
form is strictly stronger than C++'s `== 0` check and matches the existing
`tlib` substitution guard (`aperture(id) < level`). Store `root` in
`memo.liftn` on this fast path as well, so repeated `liftn(root, threshold)`
calls hit the `liftn` table directly instead of recomputing even a memoized
aperture query.

### Phase P3 — sweep for other compound-key or piling anti-patterns (audit only)

The `property2` commit is a prompt to audit faust-rs for places that build a
*tree* just to serve as a memo key (the Rust smell equivalent of
`tree(KEY, a, b)`). Known-clean after this analysis:

- `eval_cache` (`(TreeId, EnvFrameKey)` flat key) — clean.
- `automaton_cache` (evaluated-rule-list `TreeId`) — clean.
- `PropagateMemo::liftn` (`(TreeId, i64)`) — clean.
- `sigtype`, `normalize/simplify` memo tables — keyed by `TreeId`/tuples — spot-check during implementation, expected clean.

If any site is found interning an arena node purely to obtain a memo key,
convert it to a plain-tuple `AHashMap` key and note it in `JOURNAL.md`.

---

## 5. Validation

Per project convention every phase must be **FIR-identical** and behavior-preserving:

1. `cargo fmt --all`.
2. `cargo clippy --workspace --all-targets -- -D warnings`.
3. `cargo test --workspace --all-targets` (notably
   `crates/propagate/tests/core_api.rs`).
4. `cargo run -p xtask -- golden-check`.
5. Impulse-test harness: `tests/impulse-tests` via `crates/impulse-runner` —
   baselines to hold: cpp 92/93, c 87/93, interp 74/93.
6. FIR dump identity on a corpus sample before/after each phase (the memo must
   change *time*, never *output*): include UI-heavy files (group-path
   sensitivity), `Symbolic`-producing files (slot-env correctness under P1a's
   empty-env guard), and FAD/RAD files (`fad_*.dsp` — the `suppress_fad`
   bypass and deferred-seed path).
7. A targeted grouped-UI memo regression: reuse the same widget node under two
   different normalized group paths and assert both propagated signals resolve
   to their distinct `control_ids`, proving `group_path` is part of the replay
   key.
8. A targeted clock-domain memo regression (P0.3 requirement): propagate the
   *same* box under two different clock environments (e.g. the same
   sub-circuit used inside two distinct `ondemand` instances, and once at the
   top-level rate) and assert the propagated signals are distinct and carry
   the correct domain tokens — proving `clock_env` is part of the replay key.
   Baseline shape:
   `crates/compiler/tests/ondemand_pipeline.rs::structurally_identical_ondemand_instances_get_distinct_domains`.
9. Performance: re-run the March measurement
   (`faust-rs -pn clarinetMIDI tests/demos_tests.dsp`, was 0.761 s vs C++
   0.146 s) plus one FFT-like case; report P0 profiler tables before/after in
   the journal. Expected from the March analysis: P1 ≈ 3–4×, P2 ≈ 1.3–1.5× on
   propagation-bound files.

## 6. Risks

- **Stale-context replay** is the only correctness risk class: every input the
  body reads must be in the key or force a bypass. The audit in §3 is the
  contract; any future field added to `PropagateContext` must be classified
  the same way (add a doc comment on `PropagateResultKey` saying so).
- **FAD seed side channel** (`pending_fad_seeds`) is the sharpest edge — it is
  why the memo bypass condition must be `suppress_fad || contains_forward_ad`,
  not just `suppress_fad`: a cached `fad(...)` result from a non-suppressed
  context replayed inside a Rec branch would double-expand.
- **Memory**: caching `Vec<SigId>` per distinct `(box, inputs, …)` is bounded
  by what C++ has always cached; per-call lifetime caps it. If FFT-scale files
  show pressure, borrow upstream's `Map().swap` idea → `prop = AHashMap::new()`
  on drop is automatic in Rust; nothing to do.
- **P1b complexity**: only build the slot-env interner if P0 data demands it.

## 7. Upstream references

- `cb6891ae` — make CTree/Symbol hash tables grow instead of fixed-size (`compiler/tlib/tree.{hh,cpp}`, `compiler/tlib/symbol.{hh,cpp}`, `compiler/global.{hh,cpp}`; adds `-hlf/--hash-load-factor`).
- `688c0a478` — tlib: faster Tree library — array-based `make` overloads, non-virtual `Garbageable`, tlib decoupled from `global`/`exception` into a standalone library (`compiler/tlib/*`; `29cda8807` is its Windows cleanup follow-up).
- `2aec3bff0` — tlib+propagate: lazy per-node properties, fast-path slot, tuple memoization (`compiler/propagate/propagate.cpp`, `compiler/tlib/tree.{hh,cpp}`).
- `0ef3eb314` — property2\<Tree\> binary memoization (`compiler/tlib/property.hh`, `compiler/evaluate/eval.cpp`, `compiler/global.{hh,cpp}`).
- Prior faust-rs analysis: [propagation-performance-analysis-plan-2026-03-24-en.md](propagation-performance-analysis-plan-2026-03-24-en.md) (Phase 1a/1b deferred; superseded by this plan).
- Draft patch (does not apply post module-split, kept for reference): `0001-perf-propagate-memoize-propagate_in_slot_env-liftn-a.patch` at repo root.
