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

## 4. Outcome (2026-10-25)

Phases 1 and 2 are implemented. Same measurement as §1 (`--compilation-time`,
C++ backend):

| program | total before | total after | `propagation` | `fir-prepare-normalize` |
|---|---|---|---|---|
| FIR 1024 plain | 0.40 s | 0.12 s | 0.003 s | 0.33 → 0.012 s |
| FIR 4096 plain | 6.35 s | 0.92 s | 0.013 s | 5.51 → 0.049 s |
| FIR 4096 `fad` | 8.26 s | 2.60 s | 1.67 s | 5.66 → 0.055 s |
| FIR 2048 `rad` | 3.23 s | 0.89 s | 0.008 s | → 0.47 s |
| IIR 64 `fad` | 0.95 s | 0.49 s | 0.19 → 0.015 s | 0.54 → 0.25 s |
| IIR 128 `fad` | 6.63 s | 2.38 s | 1.59 → 0.072 s | 3.53 → 1.19 s |
| IIR 256 `fad` | — | 12.0 s | 0.33 s | 5.38 s |

At larger sizes, two costs remain, both outside these phases:

- **The FIR under `fad` in propagation** is quadratic: 6.9 s at 8192 taps,
  the dense bundles of §2.3, i.e. Phase 3.
- **The FIR's evaluation** is quadratic too: 2.1 s at 8192 taps, plain.
  That is `fi.fir`'s recursive list pattern, independent of AD and not
  analysed here.

For the IIR under `fad`, the preparation that remains is mostly the
retyping passes, linear in an O(N²) graph.

Phase 2 extends only an appended term: a term whose signature comes before
the last one of the sum takes the full path (§3, condition 3). On `fi.fir` and
`fi.iir`, signatures are appended. The order-64 IIR under `fad` refused 113
extensions out of 14,704.

Validation:

- **Debug checker.** Every fast-path result is recomputed by the full path,
  which must return the same node and create none. It was active over the
  whole debug workspace suite (3,435 tests, 0 failures).
- **`sum_chain` unit tests,** including two mutations the checker rejects.
- **Codegen-identity differential.** All 632 `.dsp` files of `tests/` and
  `examples/`, in scalar, `-vec` and `-double` modes, through the base
  (`22999fe5`) and the new binary. All 1,896 compilations are identical in C++
  text, exit status and stderr; 115 of them fail identically on both
  (timeouts and refused programs).
- **Compile-budget A/B by hand,** over the 13 basket cases, best of five: the
  new/base ratios are 0.96 to 1.02, which is noise.

## 5. The `fi.tf2` cascade under `fad` (analysis, 2026-10-25)

Phases 1 and 2 left the cascade of K `fi.tf2` sections (5K slider seeds)
unchanged: 64 sections compile in about 280 s through `faustprobe`. This
section records why, and what would fix it. Only the measurements and the
`sigtype` prototype below were made; nothing else is implemented.

### 5.1 Where the time goes

`faust-rs --compilation-time`, C++ backend, at `e8ac891d`:

| K | total | `fir-prepare-normalize` | C++ lines |
|---|---|---|---|
| 8 | 0.17 s | 0.08 s | 3,321 |
| 16 | 0.68 s | 0.48 s | 12,281 |
| 32 | 4.83 s | 3.67 s | 47,481 |
| 64 | 42.8 s | 33.1 s | 187,001 |

The output is quadratic (×4 per doubling). Section j carries the tangents of
every seed upstream of it, so this is the forward-mode contract, as for the
IIR. The front end is cubic (×9 per doubling). Through `faustprobe` most of
the 280 s is neither: at K = 32, `-lang cranelift --compilation-time` gives
`cranelift-codegen` 7.0 s against 2.5 s for everything before it.

### 5.2 Cause 1: the Cranelift register allocator (the dominant cost)

Profiling `-lang cranelift` at K = 32 puts 91 % of the samples in
`regalloc2::ion::run`, the default `backtracking` allocator, on the one large
`compute` function. Its cost grows much faster than the function: about ×35
for ×4 of code from K = 32 to K = 64.

Cranelift 0.135 has `regalloc_algorithm = "single_pass"`. A temporary
experiment set it through an environment variable in `make_jit_builder`
(`crates/codegen/src/backends/cranelift/core.rs`) and was reverted.
`faustprobe --double --block 256 -n 48000 --time`:

| program | compile, backtracking → single_pass | compute (× real time) |
|---|---|---|
| tf2 cascade K = 32, `fad` | 11.9 → 3.7 s | 0.55 → 0.59 |
| tf2 cascade K = 64, `fad` | 274 → 28 s | 0.12 → 0.13 |
| `fi.iir` order 256, `fad` | 90 → 17 s | 0.06 → 0.03 |
| `fi.fir` 4096 taps, `fad` | 7.4 → 3.9 s | 1.12 → 1.04 |
| `fi.fir` 1024 taps, `fad` | 0.50 → 0.33 s | 5.66 → 5.50 |

At K = 64 the JIT goes from about 250 s to about 4 s; the rest is the front
end. The generated code is not always slower, but it can be, by 2× on the
order-256 IIR. Choosing the allocator is therefore a trade between compile time
and run time, so it is a decision for the user. Two options:

- an explicit option on `faust-rs` and `faustprobe`, with the default
  unchanged;
- a threshold on the function size above which `single_pass` is used, with
  the option to force either.

Either changes only the machine code Cranelift emits, not the FIR. There is
no C++ counterpart to keep in parity.

### 5.3 Cause 2: the fixpoint of the recursion types (front end)

`fir-prepare-normalize` is mostly the retyping passes, through
`TypeAnnotator::annotate`, `solve_recursive_groups` and `SeedWalk`. A
temporary counter (reverted) at K = 64 gave, per annotation:

- 64 groups;
- 66 iterations;
- 2,333 body inferences;
- 3.3 s, 1.9 s of it in `SeedWalk::invalidate`.

About seven annotations run per compile.

The fixpoint is the Jacobi iteration of C++ `typeAnnotation`: each iteration
recomputes the groups whose seeds changed, from the previous iteration's
approximations. In a chain of K groups a change moves one group downstream
per iteration, so the iteration runs K + 2 times. Group j is recomputed about
j times, and its body has O(j) slots under `fad`, so the cost is Σ j² = O(K³).
The incremental `SeedWalk` already skips the groups that did not change. What
remains is the wavefront itself.

Two exact costs inside it:

- **`infer_proj` cloned the whole tuple of a group per projection.**
  `TypeAnnotator::infer` returns a clone of the memoised type, so a body of S
  slots cost O(S²) in clones.
  - A prototype reads the group's type in place and clones the projected
    component only: `fir-prepare-normalize` at K = 64 goes from 33.1 to
    23.8 s, and the compile to C++ from 42.8 to 32.6 s.
  - The C++ output is identical at K = 16, 32 and 64. The codegen-identity
    differential over the 632 `.dsp` × 3 modes against `e8ac891d` gives
    1,896/1,896 identical. `sigtype`'s 63 tests pass.
  - The prototype is in the working tree, **not committed**.
- **Hashing.** `in_progress` and the `SeedWalk` sets use SipHash. AHash took
  K = 64 further from 23.8 to 20.8 s (−13 %). That is too little to add a
  dependency to `sigtype` without a wider A/B, so it was reverted.

`identical()` compares two equal types by their `Debug` text, about 9 % of
the samples. That could be a structural comparison including `lsb` and `res`.

**The real fix would be a different schedule.** The schedule would visit the
groups in dependency order, as Gauss–Seidel over the condensation of the group
graph, so that each group is recomputed after its upstream groups have
settled. The chain would then take a few passes instead of K. With
`RECURSIVE_WIDENING_LIMIT = 0`, a bound that changes jumps to its upper bound
at once. If the inference is monotone, the set of bounds that change, and so
the result, should not depend on the schedule. But this is an argument, not a
proof. The iteration count and the widening ages would differ from the C++
reference, which the current code reproduces on purpose
(`solve_recursive_groups_jacobi` is its test oracle). So it needs the user's
decision and a differential on the inferred types (Rust against itself across
the corpus, and the types against C++ on the clocked and AD fixtures) before
any code.

### 5.4 Measurement correction for §4 and the synthesis §14

The `faustprobe` matrix behind the "before → after" tables of
`docs/fad-rad-synthesis-{en,fr}.md` §14 ran with two orphan `faustprobe`
processes busy. They came from the `tf2_fad_256` cases of earlier attempts,
whose timeout killed `/usr/bin/time` but not its child. The ratios hold, but
the absolute values are somewhat high.

The clean rerun (base `22999fe5` against `e8ac891d` plus the `infer_proj`
prototype, nothing else running but Spotlight):

| program | fad compile, base → new | rad compile, base → new |
|---|---|---|
| `fi.fir` 1024 | 0.89 → 0.49 s | 1.74 → 0.93 s |
| `fi.fir` 4096 | 13.9 → 7.1 s | 34.2 → 20.5 s |
| `fi.iir` 64 | 2.06 → 1.29 s | 0.07 → 0.06 s |
| `fi.iir` 256 | 140 → 93 s | 0.48 → 0.33 s |
| `fi.tf2` × 64 | 289 → 280 s | 0.22 → 0.19 s |
| `fi.tf2` × 256 | over 600 s | 3.4 → 2.2 s |

§14 still shows the earlier figures and should take these when the
`infer_proj` fix is committed.

### 5.5 Next steps, in order of payoff

1. **The Cranelift allocator choice** (§5.2), once the user decides: the
   largest gain, at K = 64 the JIT goes from ~250 s to ~4 s.
2. **Commit the `infer_proj` fix** (§5.3), exact, with §14 corrected
   (§5.4).
3. **The dependency-ordered fixpoint** (§5.3), if the user accepts a schedule
   different from C++, behind a differential on types.
4. **Phase 3** (sparse tangent bundles) and the evaluation of `fi.fir`
   (§4), for the long FIR.

## 6. Non-goals

- Changing the normal form itself: a different factorization, or a different
  term order. Those are parity decisions, and DIFF-BEH-023 is the only one in
  this area.
- The output size of `fad` on an IIR: O(N²) is the forward-mode contract.
  `rad` stays the tool for high orders.
