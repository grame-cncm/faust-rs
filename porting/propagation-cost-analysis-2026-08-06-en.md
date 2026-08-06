# Propagation cost — analysis with `virtualAnalogForBrowser.dsp` as reference

**Date**: 2026-08-06
**Status**: analysis; nothing implemented
**Reference case**:
`/Users/letz/Developpements/Recherche/WAC/WAC 2017/Faust/virtualAnalogForBrowser.dsp`
(331 lines, 108 UI widgets), reported by Stéphane as far more expensive under
faust-rs than the impulse corpus suggests.
**Related**: `porting/cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md`
(the fix this points to), `porting/compile-time-provenance-regression-analysis-and-plan-2026-07-30-en.md`
(a residue of which is measured here),
`porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md` (whose
§7 this corrects for the second time).

---

## 1. Why the corpus could not have found this

The 133-DSP impulse corpus puts `propagation` at **2.2 %** of compile time. On
this DSP it is **82 %**. Every conclusion in this repository that bounded
propagation work by its corpus share — including two of mine — is invalid for
programs shaped like this one, and the corpus contains nothing shaped like it.

That is the first finding, and it outranks the rest: **the corpus is not a
representative sample for this stage.** A propagation change must be measured
against a case like this one, and this document nominates this DSP as that case.

## 2. Measurement

```
faust-rs -time virtualAnalogForBrowser.dsp     13.40 s
faust     -time virtualAnalogForBrowser.dsp     4.05 s
```

| stage | faust-rs | C++ Faust 2.87.1 | ratio |
|---|---|---|---|
| **propagation** | **10.64 s** | 0.27 s | **39×** |
| evaluation | 1.68 s | 3.25 s | **0.52×** |
| arity | 0.64 s | — | |
| parser | 0.016 s | 0.0014 s | |
| everything after propagation | 0.03 s | 0.04 s | |

Two things worth separating. **Evaluation is now roughly twice as fast as the
reference** on this input, after the lexer work of the same day — the front end
is no longer the problem. And the entire remaining gap is one stage.

### 2.1 Where propagation time goes

Self time during propagation (`sample`, "Sort by top of stack"):

| frame | samples |
|---|---|
| allocator (`tiny_free_list_add_ptr`, `tiny_malloc_*`, `free`, …) | **~1694** |
| `boxes::matcher::match_box` | 273 |
| `propagate::SignalOrigins::record_outputs` | 201 |
| `propagate::engine::propagate_inner` | 112 |
| `propagate::arity::box_arity_typed` | 111 |
| `propagate::flat::flat_node_kind` | 108 |
| `propagate::SignalOrigins::record_derived_forest` | 79 |

Allocation churn is roughly half of it, which is a symptom rather than a cause:
the work being repeated allocates.

### 2.2 Provenance is 39 % of the stage

Switching `propagate_typed`'s `SignalOrigins::default()` to
`SignalOrigins::disabled()` — a diagnostic experiment, reverted:

| | with provenance | without |
|---|---|---|
| propagation | 10.64 s | **6.45 s** |
| total | 13.40 s | 8.90 s |

So provenance recording costs **4.2 s**, and this is the "Cause 2 —
per-propagation provenance forest walk (dominant residue)" of the 2026-07-30
plan, still live. That plan is marked executed and took the *corpus* from 4.53×
to 1.10×; its residue was measured on the corpus, where it is invisible.

**Even with provenance off, propagation is 6.45 s against 0.27 s — still 24×.**
Provenance is a real cost but not the mechanism.

## 3. The mechanism

Not widget count, not sharing depth, not any single library function:

| probe | result |
|---|---|
| 64-widget linear chain | 0.16 ms |
| 14-level shared-subtree doubling | 0.05 ms |
| `ve.moog_vcf_2bn` with slider arguments | 5.5 ms |
| the same wrapped in `ba.bypass1` | 5.6 ms |

The cost appears when a **large argument expression is used several times
inside the callee**. Feeding a 120-operation expression to a function that uses
its parameter `K` times:

| K | faust-rs | C++ Faust | faust-rs vs K=1 | C++ vs K=1 |
|---|---|---|---|---|
| 1 | 0.501 ms | 0.831 ms | 1.00 | 1.00 |
| 2 | 0.633 ms | 0.577 ms | 1.26 | 0.69 |
| 4 | 1.078 ms | 0.687 ms | 2.15 | 0.83 |
| 8 | 2.023 ms | 0.619 ms | 4.04 | 0.74 |
| 16 | 4.103 ms | 0.693 ms | **8.19** | **0.83** |

**C++ is flat in `K`; faust-rs is linear in it.** The reference propagates a
shared argument once and reuses the result; faust-rs re-propagates the whole
expression at every use.

That is exactly the gap
`porting/cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md` names as its
main one — no result memo in `propagate_in_slot_env` — and it is why this DSP
costs 39×: its arguments are large (envelopes, glide, `select2` trees), used
many times, and nested, so the factor compounds rather than adds.

## 4. Correcting this repository's record

Twice now I have bounded the value of that plan by the wrong quantity:

1. In `eval-box-simplification-memoization-analysis-2026-08-06-en.md` §7, by the
   `propagation` stage's 2.2 % corpus share. Wrong because a stage name does not
   locate a function — `propagate_in_slot_env` also runs during evaluation.
2. In the same document's P2 correction, by replacing that with "the self-time
   profile does not show that function among the leaders". Wrong because the
   profile was of a corpus DSP, and the corpus does not contain this shape.

The honest statement is that **that plan's value was never measured on an input
that could show it**, and on the first such input it is worth up to 24× on the
dominant stage.

## 5. What to do, in order

1. **Adopt this DSP as the propagation reference case.** Add it (or a reduced
   equivalent that keeps the argument-reuse shape) to a measurement harness, so
   the next claim about propagation is checkable. `xtask compile-profile` takes
   a corpus directory; it needs to be pointed at this too.
2. **Implement the `propagate_in_slot_env` result memo** per the 2026-07-04
   plan, whose §3 already works out why the naive key is unsound — the
   `suppress_fad` side channel — and prescribes bypassing the memo rather than
   widening the key. Expected: the `K` scaling flattens. Its `liftn` companion
   (§P2) was measured on 2026-08-06 and is *not* worth doing.
3. **Then re-measure provenance.** Its 4.2 s is partly the same repeated work;
   how much survives a propagation memo is not predictable from here.

## 7. Attempted 2026-08-06: the `propagate_in_slot_env` memo does not pay

Step 2 of §5 was implemented and reverted. Recorded here because the reasoning
that led to it was sound and still produced nothing, which is worth more than
the code would have been.

| variant | propagation | output |
|---|---|---|
| baseline | 10.6 s | — |
| P1a — memoize only when the slot env is empty | 13.7 s | byte-identical |
| P1b — slot env in the key, `contains_forward_ad` memoized | 13.9 s | byte-identical |

Three measurements explain it:

1. **88 % of calls have a non-empty slot env** (2.11 M of 2.4 M), inverting the
   assumption P1a is built on. `suppress_fad` and `ForwardAD` subtrees block
   *zero* calls on this input, so the plan's carve-outs cost without excluding.
2. **Slot envs are tiny** — mean 1.97 bindings, max 4 — so the plan's P1b
   interning machinery is unnecessary; sorted bindings in the key are cheaper
   and exact. That makes every call memoizable.
3. **The hit rate is then 12 %**, with the table at **748 k entries** to avoid
   123 k recomputations. The same box is rarely propagated twice with the same
   inputs.

So the repeated work that §3 measures — C++ flat, faust-rs linear in the number
of uses of a shared argument — **is not repeated at the granularity of
`propagate_in_slot_env` calls keyed by their inputs.** The next investigation
has to find where it *is* repeated, rather than assume this function is the
place; the profile's allocator dominance (~50 % of self time) is the better
lead, and a memo that adds 748 k `Vec<SigId>` entries moves it the wrong way.

`contains_forward_ad` memoization is worth keeping in mind independently: it is
a pure structural predicate over `FlatBoxId` and the probe made it hot. It is
not worth landing on its own, because nothing else calls it per node today.

## 6. Validation obligations

Whatever is implemented must carry these, and none of them is satisfied by the
corpus alone:

| # | Obligation | Check | Rejecting mutation |
|---|---|---|---|
| P1 | Identical signal output | Full impulse corpus, every backend, plus this DSP compiled and diffed against its current output | Return a stale memo entry for one node kind |
| P2 | Sound under `suppress_fad` | The FAD/RAD corpus, and the carve-out of the 2026-07-04 plan §3 | Put `suppress_fad` in the key instead of bypassing → seeds are dropped |
| P3 | The scaling law changed | The `K`-scaling probe of §3 must go flat, like C++ | — (that is the point) |
| P4 | No cross-compilation reuse | The memo is per-compilation; same obligation and same test shape as the arena memo of the box-simplification work | Hoist it to a `thread_local` |

P4 is not hypothetical: the box-simplification memo of the same day failed
exactly that way, and only a test compiling twice in one process could see it.

## 8. The allocator — measured 2026-08-06, and it is the largest single item

§2.1's profile put the platform allocator at roughly half of propagation self
time, and §7 noted it as the better lead. It is.

### 8.1 Volume

`propagate_inner` is called **~30 million times** on this 331-line DSP, and its
`Vec<SigId>` result has:

| | |
|---|---|
| mean length | **1.19** |
| length ≤ 1 | **83 %** |
| length ≤ 2 | 98 % |
| length ≤ 4 | 100 % |

Thirty million heap allocations, essentially all of one element.

### 8.2 Throughput

Swapping the CLI binary's global allocator to `mimalloc` — three lines, no
other change:

| | system allocator | `mimalloc` |
|---|---|---|
| `virtualAnalogForBrowser.dsp` propagation | 10.95 s | **5.42 s** |
| — total | 13.40 s | **7.60 s** |
| impulse corpus (`compile-bench`) | 1.21× C++ | **0.82×** |
| — DSPs faster than C++ Faust | 8 of 94 | **22 of 94** |
| — median per-DSP delta | +82 % | **+36 %** |

Output byte-identical; cpp impulse lane 94/94, `golden-check`,
`cli-transcript-check`, workspace tests and clippy all unchanged.

In aggregate the corpus is now **faster than the reference** (4.05 s against
4.92 s). The median stays positive because the win scales with allocation
volume: large programs gain, trivial ones are dominated by fixed costs.

### 8.3 What this is and is not

It is a real and large win, and it is the correct choice for the binary: the
CLI is the product, and the platform allocator on macOS is simply slow under
this much churn.

It is **not** a fix for the underlying shape. Thirty million one-element
allocations remain; `mimalloc` just services them faster. That matters because
the allocator is scoped to the binary on purpose — `compiler` does not carry it
as an ordinary dependency, since a library must not impose an allocator on its
consumers — so every FFI embedder still pays the platform allocator in full.

### 8.4 `SmallVec` was tried and is a pessimization

The obvious follow-up — `SmallVec<[SigId; 2]>` for propagation results, removing
~98 % of the allocations outright and helping every consumer rather than only
the CLI — was implemented and measured. It is **slower**, in both allocators:

| result type | system allocator | `mimalloc` |
|---|---|---|
| `Vec<SigId>` | 10.7–11.4 s | **5.42 s** |
| `SmallVec<[SigId; 2]>` | **15.3 s** | 5.50 s |

Same build, same DSP, byte-identical output. The inline capacity was chosen on
evidence — `SmallVec<[SigId; 2]>` is 24 bytes, exactly `Vec<SigId>`'s size, so
covering 98 % of results inline costs nothing in footprint — and the choice was
not the problem.

The problem is that these values are *moved* far more often than they are
allocated. Thirty million results are constructed, returned through several
frames, and dropped; a `SmallVec` move and drop must branch on the
inline-versus-spilled discriminant where a `Vec` move is an unconditional
three-word copy, and `with_capacity(n)` for `n > 2` spills immediately and pays
the overhead on top of the allocation it did not avoid. Both allocators already
have fast paths for 4-byte allocations, so what was saved was small and what
was added was paid 30 million times.

**The allocation count is therefore not the lever it appears to be.** What
`mimalloc` buys is not fewer allocations but cheaper ones, and that is
apparently most of what was available. A structural fix would have to reduce
how many propagation *results* exist at all, not how each one is stored.
