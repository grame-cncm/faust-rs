# Vector-mode loop separation — design options and plan

**Date**: 2026-07-09
**Branch**: `ondemand-vec-fad-synthesis`
**Scope**: `crates/transform/src/signal_fir/` (lowering + `loop_graph`),
`crates/compiler` (V6 oracle).
**Related**: [`vector-mode-analysis-port-plan-2026-06-10-en.md`](vector-mode-analysis-port-plan-2026-06-10-en.md)
(the V1–V6/D1–D2 plan; this note details the remaining "loop separation" step),
`docs/ondemand-fft-spectral-comparison-en.md`.

---

## 1. Where we are (P6 done through V5b)

The vector-mode substrate is built and every slice is locked by the V6 oracle
(scalar vs `-vec`, bit-exact) with scalar codegen byte-untouched (190 goldens):

- **V1** — `-vec`/`-vs`/`-lv` option + CLI plumbing (`ComputeMode`).
- **V2** — `loop_graph.rs`: `LoopGraph` / `LoopId` / `LoopKind {Vectorizable |
  Recursive | Island}` / `LoopNode {kind, is_reverse, pre/exec/post, deps}` +
  deterministic `topological_order` (the `sortGraph` port).
- **V3** — `needs_separate_loop` (the C++ `needSeparateLoop` table) as a pure
  tested function.
- **V4** — emission routed through the `LoopGraph` (bit-exact seam).
- **V5** — the `-vec` chunk driver (`for (vindex …) { vend = min(…); for (i0 =
  vindex …) { body } }`), bit-exact, vectorizes state-free inner loops.
- **V5b** — `slice_has_persistent_state` classification: vector mode chunks only
  state-free slices; a fully-recursive slice stays a plain serial loop.

**What is missing is the payoff:** *per-statement* separation — splitting a
recursive DSP's slice into a serial loop for the recursive core and vectorizable
loops for its pre/post parts, connected by chunk buffers. That is where SIMD
actually helps a stateful graph.

## 2. The constraint that rules out a naive post-pass

By the time a slice reaches the FIR it is a **fused loop-carried chain**. For
`process = (_ * 2 : + ~ _) * 0.5`:

```c
for (i0 …) {
    float fRecCur = fRec + 2.0f * input0[i0];   // recursive (reads fRec)
    output0[i0]   = 0.5f * fRecCur;             // "pure", but depends on fRecCur
    fRec          = fRecCur;                     // recursive (writes fRec)
}
```

The output write depends on `fRecCur`, which depends on the previous iteration's
`fRec`. To vectorize the output, all `fRecCur[i]` must first be produced serially
into a **chunk buffer**, then `output[i] = 0.5 · buf[i]` vectorizes. Recovering
this split *after* fusion means re-deriving the data-dependence graph from
temp-sharing and rematerializing — fragile and easy to get subtly wrong (the
class of bug the whole AD/vector effort must avoid). The split has to be decided
*before* the statements are fused, i.e. from the **signal graph**.

## 3. Design options

### Option A — port the C++ `VectorCompiler` (loop-aware lowering)

C++ `VectorCompiler : ScalarCompiler` overrides how each signal is compiled: a
current-loop stack (`openLoop`/`closeLoop`), `needSeparateLoop` per signal, cross-
loop values materialized as chunk buffers, dependency edges recorded on the CS
memo-cache hit.

- *Pro*: faithful, complete, upstream-shaped.
- *Con*: it interleaves loop control-flow into the core lowering. In faust-rs the
  **one** lowering serves both scalar and vector, so weaving `openLoop`/`closeLoop`
  into `core_lowering` risks the scalar path (which must stay bit-exact). C++
  sidesteps this with a subclass; faust-rs has no such split.

### Option B — a FIR-level scheduling post-pass

Rebuild a data-dependence graph over the fused statements, find the loop-carried
SCCs (cycles through `Struct` state), and re-schedule into loops with buffers.

- *Pro*: no change to the lowering.
- *Con*: §2 — the information is degraded after fusion (shared temps, in-place
  state), so reconstruction is fragile. Rejected as the primary path.

### Option C — dual lowering (separate vector path)

Keep the scalar lowering untouched; add a parallel vector-mode lowering.

- *Pro*: scalar bit-exactness is free (its code never changes).
- *Con*: large duplication of a big module; two paths to keep in sync forever.

### Option D — **signal-level loop assignment + region-routed emission (recommended)**

This is the faust-rs-native method: it **mirrors the clock-domain machinery that
already exists**. Clock domains do `clk_env::annotate` (assign each signal an
env) → `hgraph::{build_hgraph, schedule}` (a dependency graph of domains,
levelized) → lowering emits each signal into its domain's guarded block via the
region system. Loop separation is structurally identical:

| Clock domains (exists) | Loop separation (this plan) |
|---|---|
| `clk_env::annotate` → env per signal | a `loop_env` pass → **loop id per sample signal** (via `needs_separate_loop`) |
| `hgraph` (domain dependency graph) | the **`LoopGraph`** (built in V2) |
| `schedule` / levelize | `topological_order` (built in V2) |
| lower into guarded blocks (regions) | lower into **chunk loops**, cross-loop values → **chunk buffers** |

So V2's `LoopGraph` + V3's `needs_separate_loop` are already the loop analog of
`hgraph` + `clk_env`. The remaining work is:

1. a **signal-level pre-pass** assigning each sample-rate signal to a loop id
   (pure analysis on the prepared signal DAG, like `clk_env` — testable in
   isolation);
2. the lowering **consults that assignment** to route each signal's statements
   into the right `LoopNode.exec`, and inserts a **chunk buffer** store/load
   whenever a value crosses a loop boundary.

- *Pro*: reuses the proven analysis→schedule→route pattern; the analysis is a
  pure, unit-testable pass; the lowering change is *routing by a precomputed map*
  rather than restructuring its control flow; scalar stays the degenerate
  "one loop for everything" assignment already routed bit-exactly in V4.
- *Con*: the lowering must learn to emit a value into a buffer at a loop boundary
  and read it back — the one genuinely new mechanism (chunk buffers).

**Recommendation: Option D.** It is the least-risk path to a *correct* separation
because it decouples the decision (pure analysis) from the emission (routing), and
it slots into infrastructure faust-rs already trusts.

## 4. Chunk buffers — the one new mechanism

A sample value produced in loop A and consumed in loop B becomes an array of
`vec_size` (C++ `Vector*`/`Zec*`; delayed reads `Yec*`):

- **declare**: a `vec_size`-element stack array (short) or a struct-field ring
  buffer (if it must persist across chunks — e.g. a delayed cross-loop read).
- **store** (in A's `exec`): `buf[i0 - vindex] = value;` (chunk-local index).
- **load** (in B's `exec`): `buf[i0 - vindex]`.

The `i0 - vindex` chunk-local index keeps V5's "global `i0`, no I/O rebasing"
property, so the pre/post loops stay bit-exact. Delay lines that already live in
the struct keep their vector layout work in the base plan's V4 (copy dual-buffer /
ring) — orthogonal and later.

## 5. Slices (each gated by the V6 oracle, scalar untouched)

1. **S-A — `loop_env` analysis (pure, no codegen change).** A pass over the
   prepared sample-rate signals that returns `SigId → LoopId` (via
   `needs_separate_loop`) plus the loop dependency edges, populating a `LoopGraph`
   *shape* (no statements yet). Unit-tested on hand-built signal graphs exactly
   like the `hgraph` tests. Zero runtime effect.
2. **S-B — routing with a single loop (identity check).** Make the lowering
   consult S-A's map but with every signal mapped to one loop → must reproduce the
   current one-loop-per-slice emission bit-for-bit (goldens + oracle). This proves
   the routing seam before any real split.
3. **S-C — chunk buffers.** Add the buffer declare/store/load mechanism (§4) and
   a `loop_graph` unit test for the buffer index arithmetic.
4. **S-D — first real split: pure tail. ✅ (2026-07-10)** Splits a recursive
   slice's state-free tail into its own vectorizable inner loop fed by a chunk
   buffer. Realized as a FIR-level partition on the fused body
   (`partition_recursive_body`): a fixpoint closes the serial core under
   "producers of temps a serial statement reads", so the tail is provably safe to
   hoist past it (§2's fragility is avoided by the fixpoint, not by pre-fusion
   assignment — the same correctness, reached from the FIR). `emit_sample_loop`
   emits `[buffers, chunk driver{serial loop, tail loop}]` for a forward
   `Recursive` slice; anything unsupported (clocked-island `If`, nothing to hoist)
   falls back to one serial loop. Bit-exact (V6 `recursive_split_pure_tail`),
   scalar untouched (190 goldens), valid C at `-O3 -Wall -Wextra`. Extend
   incrementally (pure prefix, multiple recursive groups) — the partition already
   handles them for a flat body.
5. **S-E — measure. ✅ (2026-07-10)** Confirmed on arm64 / Apple clang (NEON):
   the split's tail loop vectorizes (`-Rpass=loop-vectorize`: width 4 interleaved
   4 for a `*0.5` tail; `fmul.4s` in the asm), while the serial core and the
   scalar single loop do not. Net speedup is **conditional** (the split adds a
   store+load per sample through `vbufN`): `1.15×` for a compute-heavy
   abs/min/max shaper tail, `0.92×` for a cheap `*0.5` tail — a small regression
   when the vectorizable work doesn't clear the buffering overhead. So a **cost
   heuristic** (split only when the tail's op count is high enough, à la Faust's
   vectorization policy) is the natural follow-up before making the split
   unconditional. Results are always bit-exact (oracle-locked); only speed moves.

Reverse-time (RAD/BRA) slices keep forcing scalar mode throughout (V5 caveat).

## 6. Validation

- **Bit-exactness** stays the primary oracle: `crates/compiler/tests/vector_mode.rs`
  (scalar vs `-vec`, byte-identical) grows a case per split shape; scalar goldens
  guarantee the scalar path never moves.
- **Determinism**: loop ids and buffer names are insertion-ordered (already true
  of `LoopGraph`), so `-vec` output is reproducible.
- **No upstream oracle** for faust-rs's loop shapes (as for FAD × domains); the
  differential vs `faust -vec` from the base plan's V6 covers *base* vector mode
  only and is a secondary check.

## 7. One-line summary

Do **not** reconstruct loops after fusion; instead add a **signal-level loop
assignment** pass — the loop analog of `clk_env`, feeding the `LoopGraph` we
already built — and have the lowering *route* statements into loops and insert
chunk buffers at loop boundaries. It reuses the clock-domain analysis→schedule→
lower pattern, keeps the decision pure and testable, and keeps scalar bit-exact by
construction.
