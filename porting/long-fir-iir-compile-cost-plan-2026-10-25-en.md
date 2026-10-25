# Long FIR and IIR under `fad`: where the compile cost goes, and the plan

Date: 2026-10-25. Base: `main-dev` at `22999fe5`.
Origin: `docs/fad-rad-synthesis-en.md` §14 (2026-09-23) reports `fad` compile
times quadratic in the FIR length and worse in the IIR order (4096 taps in
281 s, order 256 not compiled), and attributes them to the derivative: "each
derivative lane is itself an N-tap FIR, so the graph has N² nodes".

## 1. Measurements

`faust-rs` release build, `--compilation-time`, `-o /dev/null` (C++ backend, so
no Cranelift JIT in the figures), Apple M-series laptop. Programs: `fi.fir` and
`fi.iir` of the installed `stdfaust.lib` over slider coefficients, the
coefficients as seeds; `plain` is the filter without AD.

| program | total | `propagation` | `fir-prepare-normalize` | lines of C++ |
|---|---|---|---|---|
| FIR 1024 plain | 0.40 s | 0.003 s | 0.33 s | 4,215 |
| FIR 2048 plain | 1.59 s | — | 1.34 s | — |
| FIR 4096 plain | 6.35 s | 0.012 s | 5.51 s | 16,503 |
| FIR 4096 `fad` | 8.26 s | 1.68 s | 5.66 s | 28,790 |
| FIR 2048 `rad` | 3.23 s | — | — | 34,928 |
| IIR 64 `fad` | 0.95 s | 0.19 s | 0.54 s | 2,060 |
| IIR 128 `fad` | 6.63 s | 1.59 s | 3.53 s | 3,980 |
| IIR 256 plain | 0.08 s | — | — | 2,180 |
| IIR 256 `rad` | 0.20 s | — | — | 8,584 |

The pinned C++ reference (2.84.3) on the plain FIR: 12.5 s at 1024 taps,
100.6 s at 2048, and its 120 s timeout at 4096.

What this shows, against §14:

- **The FIR cost is not the derivative.** The plain FIR is already
  quadratic (×4 per doubling), and 87 % of it is `fir-prepare-normalize`. A
  tangent lane of `Σ cᵢ·x@i` with respect to `c_k` is `x@k`, one node; the
  `fad` output is linear in N (28,790 lines at 4096). §14's 281 s were
  measured through `faustprobe`, so they include the Cranelift compilation
  of a very large function, and they predate several front-end fixes.
- **The IIR under `fad` is super-quadratic** (×5 to ×7 per doubling). Its
  output is genuinely O(N²) (2N + 1 tangents, each a recursion of order N),
  but two passes cost more than the output:
  - `propagation`, ×8 per doubling, cubic;
  - `fir-prepare-normalize`, also cubic: N + 1 sums of N terms, each sum
    quadratic.

## 2. Causes

### 2.1 The de Bruijn check of the AD outputs (propagation)

`generate_fad_signals_multi` and `generate_rad_signals` check every output with
`is_de_bruijn_closed` and `check_de_bruijn_coherence`, each with a fresh memo.
The outputs share most of their DAG (every tangent of a recursion reads the
primal's state), so the whole DAG is walked once per output: O(outputs × DAG),
N³ for the IIR. Profiling `IIR 128 fad`: `tlib::recursion::check_coherence_at_depth`
and `aperture` are half of the propagation samples.

### 2.2 Normalization of long sums (prepare)

`simplify` rewrites bottom-up, and normalizes every `Add`/`Sub` node with
`normalize_add_term`. A sum built by `:>` is a left chain, so each partial sum
`s_k = s_{k-1} + t_k` is normalized in full:

- `Aterm::from_sig` decomposes the k terms;
- `greatest_divisor` indexes them;
- `normalized_tree` clones them and folds them back into a tree.

That is O(k) per prefix, O(N²) per sum. The C++ reference has the same
structure, with a pairwise divisor search on top (O(N³), which `DivisorPairs`
already removed on the Rust side). Profiling `FIR 4096 plain`:
`Aterm::add_sig` (decomposition), `normalized_tree` and `greatest_divisor`
together are the whole of `normalize_add_term`.

### 2.3 Dense tangent bundles over a long sum (propagation, FIR)

`fad` carries one tangent per seed on every node. Over the N nodes of an N-term
sum with N seeds, that is N² tangent slots, nearly all literal zeros, each
built and folded by `DerivativeZeroFolder` (1.68 s of `FIR 4096 fad`). The
output is linear; the work is not.

## 3. Plan

Every phase is an optimization. **Phases 1 and 2 must not change any generated
code**, including the order of terms. `SigId` order drives the `BTreeMap`
orders of `Aterm`/`Mterm` and the operand order of `simplifying_add`. So a
change must create the same arena nodes, in the same order, as the code it
replaces, and not merely equal-valued trees.

### Phase 1 — one memo for the coherence check of an AD output list

- `tlib::check_de_bruijn_coherence_with_memo(arena, root, memo)`, the
  counterpart of the existing `de_bruijn_aperture_with_memo`. The memo
  records successes only, so a shared memo reports the same first error at
  the same root as a fresh one per root.
- `propagate::forward_ad::check_ad_outputs_coherent`, used by `fad` and `rad`:
  one aperture memo and one coherence memo for the whole output list.
- Pass criteria:
  - a `tlib` test: a shared memo across roots gives the same result and the
    same first error as fresh memos;
  - the existing AD tests unchanged;
  - `IIR 128 fad` propagation from 1.59 s to under 0.1 s.
- Read-only checks: no arena node is created, so the output cannot change.

### Phase 2 — incremental normalization of a sum extended by new terms

Keep, for each sum `T` that `normalize_add_term` produced, the state that
built it, and extend that state when the parent is `T ± y`, instead of
decomposing `T` again.

State of `T` (owned by `SimplifyCache`, valid for one type map, removed when a
parent consumes it):

- `aterm`: the terms of `T`. The ones that a previous parent already
  decomposed hold the mterm that decomposition gave. The *pending* ones,
  appended when `T` was built, still hold the mterm they were built from.
- `pending_roots`: the eight bucket folds `p[ord]`/`n[ord]` of the previous
  state. A walk of `T` that stops at them reaches exactly the pending terms,
  in the order the full decomposition meets them.
- `folds`: the bucket folds `p[]`/`n[]` of `T`, before the final combination.
- `index`: the divisor candidate index (`(factor, sign)` and coefficient
  magnitude → terms).
- `last_key`: the largest signature key.

`normalize_add_term(T ± y)` takes the fast path when every condition below
holds. Otherwise it falls back to the current code, from scratch.

1. `T` has a state.
2. **Pending terms.** Walk `T`, stopping at `pending_roots`. For each pending
   term, run the decomposition the full path would run (`Mterm::from_sig` and
   its signature, the `-1` product for a negative one). The term must give back
   its own key, and a mterm whose tree equals the stored one: the same factors,
   and a coefficient that is either the same node or in the same `±1` class
   with the same sign.
3. **New terms.** Each term of `y` is decomposed as the full path would. Its
   key must be absent from the state, and greater than `last_key`, so the term
   is appended.
4. **Divisors.** Each new term, against its candidates in `index` and the new
   terms before it, has a gcd of complexity 0. The state invariant says every
   old pair has complexity 0, so `greatest_divisor` is 1 and no factorization
   runs, as in the full path.
5. **The new state.** No zero coefficient, no `-1 · SUM` wrap, and every new
   term tree is a product or a factor (not an `Add`/`Sub`, which the parent
   would split). When this fails, the result is still returned, but no state
   is kept.

The fast path continues the bucket folds with the new term trees and combines
them as `normalized_tree` does. Exactness argument:

- The full path creates nodes only for the pending terms (their
  decomposition), for the new terms (decomposition, tree, fold step) and for
  the final combination.
- The terms already decomposed by an earlier parent are re-interned to existing
  nodes, so they create none.
- The fast path runs those same operations, in the same relative order, and
  skips only work whose nodes already exist.
- A failed check falls back before any operation the fast path would order
  differently. Every operation run before the check is one the full path runs
  at that point too.

Cost: O(new terms + log k) per prefix instead of O(k), linear per sum.

Pass criteria:

- a debug-build checker:
  - under `cfg(test)`, every fast-path result is recomputed by the full path
    on the same arena, and the two outputs must be the same node;
  - the full path, run second, must create no node;
  - mutation tests (a skipped pending check, a wrong fold) must make the
    checker fail;
- unit tests for each fallback condition and for the fast-path hit rate on
  long chains (FIR, IIR tangent sums, alternating signs);
- a codegen-identity differential: every `.dsp` of `tests/` and `examples/`
  compiled by the base and the new binary must give identical C++, at the
  scalar and `-vec` settings the corpus uses;
- `FIR 4096 plain` normalize from 5.5 s to under 0.5 s, and `IIR 128 fad`
  normalize from 3.5 s to under 1 s;
- a compile-budget A/B by hand (the gate cannot measure on this machine).

### Phase 3 — sparse tangent bundles (deferred)

Carry only the non-zero tangents of a node (seed index → tangent) instead of
a vector of N. This would make the FIR under `fad` linear in propagation as
well (2.3).

It is a representation change across `forward_ad.rs`: every rule, the
recursion augmentation, and the clock-domain twin. It is left for its own
plan, after Phases 1–2 show what remains. At 4096 taps it is 1.7 s, against
the 5.5 s of normalization.

## 4. Non-goals

- Changing the normal form itself: a different factorization, or a different
  term order. Those are parity decisions, and DIFF-BEH-023 is the only one in
  this area.
- The output size of `fad` on an IIR: O(N²) is the forward-mode contract.
  `rad` stays the tool for high orders.
