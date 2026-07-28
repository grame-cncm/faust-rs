# faust-rs vs C++ Compilation Speed: Analysis and Improvement Plan

**Date**: 2026-03-24
**Status**: Analysis complete, implementation deferred
**Subject**: ~5× slowdown on `clarinetMIDI.dsp` (faust-rs: 0.761s, C++: 0.146s)

---

## Measurement

```
$ time faust-rs -pn clarinetMIDI tests/demos_tests.dsp   # 0.761s
$ time faust    -pn clarinetMIDI tests/demos_tests.dsp   # 0.146s
```

Profiling with `cargo flamegraph` shows propagation consumes ~850 ms out of the
880 ms total.  Everything else (parsing, evaluation, codegen, FIR lowering) is
negligible.  The bottleneck is entirely in `crates/propagate/src/lib.rs`.

---

## Root cause analysis

### 1. No memoization of propagation outputs — PRIMARY BOTTLENECK

**C++ behaviour**
`boxPropagateSig` (C++ `compiler/generator/signals/sigtyperules.cpp`) calls
`setProperty(box, inputs, outputs)` / `getProperty(box, inputs, outputs)` — a
persistent, cross-call property cache on the tree node.  Whenever the same
`(box, input_list)` pair is seen again (which is very common due to hash-consed
sharing), the cached output list is returned immediately without recursion.

**Rust behaviour**
`PropagateMemo` in `propagate_inner` only caches two helpers:

```rust
pub struct PropagateMemo {
    liftn:    HashMap<(TreeId, usize), TreeId>,
    aperture: HashMap<(TreeId, usize), Option<usize>>,
}
```

The main `propagate_in_slot_env` function itself is **not memoized**.  Every
call to `propagate_in_slot_env(box, slot_env)` recomputes from scratch, even
when `(box, slot_env)` has been seen before.  For a deeply shared signal graph
this means exponential recomputation of subtrees.

**Impact**: major — likely accounts for 80%+ of the slowdown.

**Fix**: Add a `HashMap<(TreeId, SlotEnvId), Vec<TreeId>>` memo table to
`PropagateMemo` (or `PropagateContext`) and check it at the top of
`propagate_in_slot_env`.  `SlotEnvId` can be the hash of the slot-env vector or
a content-addressed key.

---

### 2. `liftn` missing `aperture == 0` fast-path — SECONDARY BOTTLENECK

**C++ behaviour**
C++ `liftn(sig, n)` checks `aperture(sig) == 0` first.  If the signal has no
free De Bruijn references (`aperture == 0`), it is closed — `liftn` is a no-op
and returns the signal unchanged immediately.

**Rust behaviour**
`liftn` in `crates/propagate/src/lib.rs`:

```rust
fn liftn(arena: &mut TreeArena, memo: &mut PropagateMemo, sig: TreeId, n: usize) -> TreeId {
    if n == 0 { return sig; }
    // recurses into all children unconditionally
    ...
}
```

The `n == 0` guard is there, but there is no `aperture(sig) == 0` fast-path.
For closed subterms (the vast majority of leaf signals) every `liftn` call still
descends into the tree.

**Impact**: major — `liftn` is called on every node during propagation.

**Fix**: Check `aperture(sig, memo) == 0` before recursing.  The `aperture` memo
already exists in `PropagateMemo`; wire it in at the top of `liftn`.

---

### 3. `NodeKind::Symbol(Arc<str>)` — O(string_length) hashing — MODERATE

**C++**: all identifiers are interned as tagged pointers (`Node` / `Tree` are
small integers).  Hashing a `Node` is O(1).

**Rust**: `NodeKind::Symbol(Arc<str>)` hashes by iterating every byte of the
string.  The arena interner then looks up by `(NodeKind, children)`, so every
intern call for a named primitive hashes the full string.  For large DSP graphs
with many named boxes this adds up.

**Fix**: Replace `Arc<str>` with a pre-computed `u64` hash (e.g. `SymbolId` —
a newtype over an interned integer index into a global symbol table).  Hashing a
`SymbolId` is then O(1).

---

### 4. `PropagateMemo` reinitialized per top-level call — MODERATE

`PropagateMemo` is created fresh inside each `propagate` top-level call.  When
`propagate` is called multiple times (e.g., once per output channel, once per
type-check pass), all memoized work from the previous call is discarded.

**Fix**: Hoist `PropagateMemo` (and the output memo described in issue 1) to the
`PropagateContext` level or pass it in as a `&mut` parameter from the caller so
it survives across multiple top-level propagation calls.

---

### 5. `NodeKey` Vec allocation for arity ≥ 3 — MINOR

`ChildList::N(Vec<TreeId>)` allocates a heap `Vec` for every node with 3 or
more children.  Most boxes are binary (2 children) and use the inline path, so
this is a minor issue, but xtended ops with many arguments still pay alloc+free
per intern call.

**Fix**: Use `smallvec::SmallVec<[TreeId; 4]>` or a bump arena for `ChildList::N`.

---

## Improvement plan

### Phase 1 — High impact, self-contained (target: close the 5× gap)

| # | Change | File | Expected gain |
|---|--------|------|---------------|
| 1a | Add propagation output memo `HashMap<(TreeId, SlotEnvKey), Vec<TreeId>>` | `propagate/src/lib.rs` | 3–4× |
| 1b | `liftn` aperture fast-path | `propagate/src/lib.rs` | 1.3–1.5× |

**Estimated result after Phase 1**: faust-rs ≈ 0.15–0.25s (on par with C++)

### Phase 2 — Medium impact

| # | Change | File | Expected gain |
|---|--------|------|---------------|
| 2a | `SymbolId` interned integer for names | `tlib/src/arena.rs`, propagation paths | 1.1–1.2× |
| 2b | Hoist `PropagateMemo` across top-level calls | `propagate/src/lib.rs`, caller sites | 1.1–1.2× |

### Phase 3 — Micro-optimisations

| # | Change | File |
|---|--------|------|
| 3a | `SmallVec<[TreeId; 4]>` for `ChildList::N` | `tlib/src/arena.rs` |
| 3b | Profile and address any remaining hotspot | TBD |

---

## Detailed fix sketch — Phase 1a (propagation memo)

The key insight: `propagate_in_slot_env(sig, slot_env)` is a pure function of
its two arguments.  The `SlotEnv` is already a `Vec<TreeId>` (or similar); its
content uniquely determines the result.

```rust
// In PropagateContext or PropagateMemo:
prop_cache: HashMap<(TreeId, u64), Vec<TreeId>>,  // key: (sig, hash_of_slot_env)

// At the top of propagate_in_slot_env:
let key = (sig, hash_slot_env(&slot_env));
if let Some(cached) = ctx.memo.prop_cache.get(&key) {
    return cached.clone();
}
// ... compute result ...
ctx.memo.prop_cache.insert(key, result.clone());
result
```

A collision-resistant slot-env hash can be computed cheaply by XOR-folding the
TreeId integers (which are already stable content-addressed keys).

---

## Detailed fix sketch — Phase 1b (liftn aperture fast-path)

```rust
fn liftn(arena: &mut TreeArena, memo: &mut PropagateMemo, sig: TreeId, n: usize) -> TreeId {
    if n == 0 { return sig; }
    // Fast-path: closed term — no free De Bruijn refs
    if aperture(arena, memo, sig) == Some(0) { return sig; }
    // ... existing recursion ...
}
```

`aperture` is already memoized in `PropagateMemo::aperture`, so this is an O(1)
hash-map lookup for previously visited nodes.

---

## Files to modify

| File | Change |
|------|--------|
| `crates/propagate/src/lib.rs` | Phase 1a: propagation output memo; Phase 1b: liftn aperture fast-path |
| `crates/tlib/src/arena.rs` | Phase 2a: SymbolId interning |
| `crates/propagate/src/lib.rs` | Phase 2b: hoist PropagateMemo lifetime |
| `crates/tlib/src/arena.rs` | Phase 3a: SmallVec for ChildList::N |

---

## References

- C++ `boxPropagateSig`: `compiler/generator/signals/sigtyperules.cpp`
- C++ memoization: `setProperty` / `getProperty` on `Tree` nodes
- Rust propagation: `crates/propagate/src/lib.rs` — `propagate_inner`, `propagate_in_slot_env`, `liftn`, `aperture`
- Rust arena: `crates/tlib/src/arena.rs` — `NodeKind`, `ChildList`, interner tables
