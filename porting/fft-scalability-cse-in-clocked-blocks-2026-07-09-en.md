# FFT scalability: CSE is skipped inside `ondemand` guarded blocks — diagnosis and correction plan

**Date**: 2026-07-09
**Scope**: `crates/transform/src/signal_fir/{cse.rs,module/build.rs,module/clocked.rs}`
**Branch context**: `ondemand-vec-fad-synthesis` (the spectral / FFT-on-`ondemand` work)
**Related**: [`fir-cse-runtime-optimizations-plan-2026-04-03-en.md`](fir-cse-runtime-optimizations-plan-2026-04-03-en.md)
(the pass this note extends), [`ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md`](ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md),
`docs/ondemand-fft-spectral-comparison-en.md`.

**Status**: **Phase A implemented 2026-07-09** (`cse.rs` per-scope recursion).
The framed FFT is now O(N log N): arithmetic ops for N=8→128 went
188/1080/6488/39048/234632 → 104/301/834/2179/5428 (**43× fewer at N=128**,
`ops/(N log N)` flat 4.3→6.1, ×/doubling ≈ 2.5); generated code at N=256 shrank
9.2 M → 60 k interp lines (~152×) and compile time 8.6 s → 1.8 s. Numerics
unchanged (interleave_fft, impulse-runner, cpp_clocked_differential all pass;
190 goldens unaffected). Phases B–D still open (see §5); N=512 still hits the
eval-budget wall.

---

## 1. Symptom

The frame-rate FFT built on `ondemand` (`tests/corpus/ondemand_fft_framed_*.dsp`)
does not scale: `N=256` compiles in ~8.6 s and 9.2 M `.fbc` lines, `N=512`
fails on the evaluator recursion budget, and the round-trip effects are heavier
still. The naive reading was "Faust unrolls everything, so large N is
expensive." That reading is **wrong about the dominant cost**.

## 2. Measurement: the generated FFT is O(N^2.6), not O(N log N)

Counting floating-point `+`/`*` in the generated C++ of the **framed**
(analysis-only, in-`ondemand`) FFT:

| N | arithmetic ops | ops / (N log₂N) | ×/doubling |
|---|----------------|-----------------|------------|
| 8   | 188     | 7.8   | —    |
| 16  | 1 080   | 16.9  | 5.74 |
| 32  | 6 488   | 40.5  | 6.01 |
| 64  | 39 048  | 101.7 | 6.02 |
| 128 | 234 632 | 261.9 | 6.01 |

~6× per doubling ⇒ **≈ O(N^2.58)** — worse than a naive O(N²) DFT. The
normalized column diverges without bound: the butterfly sharing (the entire
point of an FFT — an O(N log N) DAG of intermediate results, each with fan-out
2) is gone. For `N=32` the generated code holds only ~65 temporaries (≈ 2N, the
output bins) and **zero intermediate-stage temporaries**: the DAG is flattened
into 2N independent trees.

## 3. It is specific to the `ondemand` block

The **plain** FFT (`process = an.fft(N)`, no `ondemand`) does **not** blow up:

| N | ops | ops / (N log₂N) | temporaries | ×/doubling |
|---|-----|-----------------|-------------|------------|
| 8  | 192   | 8.0 | 61    | —    |
| 16 | 527   | 8.2 | 193   | 2.74 |
| 32 | 1 376 | 8.6 | 539   | 2.61 |
| 64 | 3 439 | 9.0 | 1 399 | 2.50 |

`ops / (N log N)` is flat (8.0 → 9.0), temporaries grow as O(N log N): **the
plain FFT is O(N log N) and CSE works perfectly.** Same core (`an.fftb`), same
compiler — the *only* difference is whether the transform sits inside an
`ondemand` guarded block. Framed `fft(32)` = ~65 temps; plain `fft(32)` = 539
temps.

## 4. Root cause (pinned)

The sharing exists at every representation level:

- **Signals are hash-consed.** `SigId = TreeId`; `TreeArena::intern`
  (`crates/tlib/src/arena.rs`) deduplicates by `(kind, children)`. In the
  `--dump-sig` of `an.fft(4)`, the sub-FFT term
  `input[2] + (1·input[6] − 0·input[7])` is one node referenced by all 8
  outputs.
- **FIR nodes are hash-consed** too (`fir::encoding::intern_tag`).
- **A CSE materialization pass exists and is correct**
  (`signal_fir/cse.rs`): it ref-counts `FirId` fan-out per bucket and wraps
  multi-referenced non-trivial nodes in `DeclareVar` + `LoadVar`. This is what
  makes the *plain* FFT O(N log N).

The defect is that **CSE never sees the statements inside the guarded block.**
`build.rs` runs CSE on exactly three flat buckets — `constants_statements`,
`control_statements`, and each `sample_loop_statements` — and the reference
counter deliberately does **not** descend into nested scopes.
`cse.rs::value_children_of` documents it:

> For **statement nodes** this returns embedded values … *but not structural
> children such as block bodies or loop bodies — those are separate execution
> scopes and should not be traversed by intra-bucket CSE.*

and for the guard itself returns only the condition:

```rust
FirMatch::If { cond, .. } | FirMatch::Control { cond, .. } => vec![cond],
```

Consequences:

- **Plain FFT** → all butterfly statements live directly in the flat
  `sample_loop_statements` bucket → CSE ref-counts them, materializes the
  shared stage results → O(N log N).
- **Framed FFT** → the butterflies live in the `then`-body of the `BoolIf`
  guard emitted by `module/clocked.rs` (the `ondemand` block). That body is a
  nested scope, so CSE visits only the guard's `cond` and skips the entire
  transform. With no `DeclareVar` temporaries inserted, the backend prints each
  hash-consed value node inline every time it is referenced → the DAG is
  re-expanded into 2N trees → **O(N^2.6)**.

So the wall is not "unrolling", not the eval budget, and not the FFT
formulation. It is a **one-line scope-coverage gap in the CSE pass**: intra-
bucket CSE stops at guarded-block boundaries, and *every* stateful `ondemand` /
`upsampling` / `downsampling` block (P3) is such a boundary. Any nontrivial
computation inside a clock-domain block currently pays the fully-inlined cost.

## 5. Correction plan

### Phase A — per-scope CSE inside guarded blocks (the fix) — DONE

*Implemented 2026-07-09.* `signal_fir/cse.rs` now runs CSE **per execution
scope**: `materialize_scope` recurses into `If`/`Control`/loop/`Block` bodies as
independent buckets (fresh ref-counts, temporaries local to the body), with the
`fTemp`/`iTemp` counters threaded through the whole scope tree for unique names.
Guard conditions and loop headers are left untouched (evaluated in the enclosing
scope). `build.rs` call sites dropped the pre-computed `ref_counts` argument.
The results above confirm O(N log N). Design as originally specified below.

Make CSE run **once per execution scope**, treating each block / loop / guard
body as its own bucket, materializing temporaries **local to that scope**
(declared inside the body, used inside the body — which is exactly the
correctness constraint the current "separate execution scopes" comment is
protecting: a temp must not be hoisted across a conditional boundary).

Concretely:

1. Add a recursive driver that, for every statement carrying a structural body
   (`If`/`Control` `then`/`else`, loop bodies, nested `Block`s), collects that
   body's statement list and runs the existing
   `count_fir_value_uses` + `materialize_shared_values` on it as an independent
   bucket, with `fTemp`/`iTemp` counters threaded through so names stay unique.
2. Recurse depth-first so inner scopes are materialized before their parents.
3. Leave the cross-scope rule intact: a node used in two sibling scopes is
   materialized independently in each (correct, and matches the current flat
   behavior). A future refinement can hoist a node used in a scope **and** its
   dominator to the dominator; not needed for the FFT.

Expected payoff: the in-block FFT drops from O(N^2.6) to **O(N log N)** in both
generated-code size and arithmetic — i.e. compile time *and* runtime improve
together. This is the single highest-leverage change and it is localized to
`cse.rs` + the call site in `build.rs` (plus, if guard bodies are not already a
walkable statement list at CSE time, a small hook in `module/clocked.rs`).

**Validation**:
- Re-run the op-count harness (§2/§3): the framed FFT's `ops/(N log N)` must go
  flat, matching the plain FFT within a small constant.
- Numerical parity unchanged: `crates/compiler/tests/interleave_fft.rs` (bins ==
  direct DFT) and the impulse-runner effect checks must still pass bit-for-bit.
- Add a regression asserting the framed FFT emits O(N log N) temporaries (e.g.
  temp count at N∈{16,32,64} grows ≈ ×2.5, not ×6).

### Phase B — lift the front-end ceiling (needed to exploit A at large N)

Even with A, box evaluation still recurses; `N=512` fails at
`FRS-EVAL-0099` (guarded depth 1024). **Interim workaround (works today):**
`FAUST_RS_DEFAULT_EVAL_MAX_DEPTH=4096` raises the budget and compiles the full
round-trip at `N=1024` (with A, ~0.5 M interp lines, ~40 s). The proper fix is to
make the evaluator iterative (explicit work stack) so no budget knob is needed,
and land the propagate-memoization port ([[project-propagate-memoization-port]])
so the two `fftb(N/2)` halves are not re-derived.

### Phase C — O(1) overlap-add output (the dominant runtime cost)

**This is the #1 runtime lever — above Phase D.** Measured on the `N=1024`
denoiser (see §6): 82 % of the per-sample cost is the `interleave_hop`
serialization harness, *not* the FFT. The culprit is `serialize_out`:

```faust
serialize_out(N, H) = par(j, N, up0(H) : @(j)) :> _;
```

This reconstructs the output stream as a **sum of N delayed, clock-masked
lanes**, evaluated **every sample**: ~N delay-line reads + N masks + N−1 adds per
output sample — **O(N) per sample**. At `N=1024` that is the ~1.75 µs/sample
floor measured with a trivial (identity) frame operator, before any FFT.

A classical STFT resynthesis is **O(1) per sample**: it keeps a single
**output accumulator ring buffer** and does

- **at each fire** (once per `hop`): `for n in 0..N { ola[(wp+n) % L] += frame[n] }`
  — an N-wide *scatter-add* of the reconstructed frame (O(N) **per hop**, i.e.
  O(N/hop) ≈ O(1) amortized per sample);
- **per sample**: `out = ola[rp]; ola[rp] = 0; rp = (rp+1) % L` — one destructive
  read (O(1)).

The whole difference is *where the N-work lives*: the Faust construction pays it
`hop`-times over (every sample), the ring buffer pays it once per hop.

**Why it cannot be a pure-Faust library fix.** The block diagram has no way to
express a shared accumulator with a *scatter-add* write:

- `par(j,N,…):>_` is inherently O(N)/sample — each lane is its own delay line,
  summed every sample. There is no "collapse the N delays into one shared
  accumulator" identity available to the diagram.
- `rwtable` does not rescue it: Faust's table model has **one write per
  activation**, so the N-wide scatter-add of a whole frame at a single fire tick
  is not expressible (it would need N write ports, or a loop the diagram cannot
  spell). The read side (moving pointer, read-and-clear) is fine; the write side
  is the blocker.

So Phase C needs one of:

1. **A dedicated `serialize_out` / OLA compiler primitive** (mirrors the
   "FFT as a structured node" fallback of Phase D): the `ondemand`/`interleave`
   harness stays, but the reconstruction lowers to a ring-buffer accumulator
   (scatter-add at fire, read-clear per sample). Cleanest, and it also fixes
   `serialize_in`'s symmetric N-tap fan if lowered the same way. Cost: it breaks
   the "pure library, no new primitive" purity of `interleave.lib` — a real
   design tension worth stating explicitly.
2. **A pattern-matching lowering** in `signal_fir` that recognizes the
   `par(j,N, up0(H):@(j)) :> _` idiom and rewrites it to the accumulator. Keeps
   the surface pure but is fragile (must match the exact masked-delay-sum shape).

Expected payoff: O(N)/sample → O(1)/sample removes ~5–6× of the runtime at
`N=1024` on its own — the largest single win, bigger than rfft + SIMD combined.

#### Phase C — concrete design sketch (option 1, the dedicated primitive)

The path mirrors `ondemand` exactly, which is already a compiler primitive, not
library sugar:

- **Surface / box → signal.** `ondemand` is `BOX_ONDEMAND_TAG`
  (`boxes/src/tags.rs`) → `SigMatch::OnDemand(&[SigId])` (`SIG_OD_TAG`,
  `signals/src/lib.rs`) → lowered in `signal_fir/module/clocked.rs`. Add the
  twin: `BOX_SERIALIZE_OUT_TAG` → a `SerializeOut { clock, hop, lanes: &[SigId] }`
  signal node. `interleave.lib` keeps its shape but its output stage calls the
  new primitive instead of `par(j,N, up0(H):@(j)) :> _`. (Type/arity: N lanes +
  the boolean fire clock in, one signal out; hop is a compile-time literal.)

- **FIR state (reuse the rwtable machinery).** The accumulator is a persistent
  ring buffer `fOla[L]`, `L = N + hop` (headroom so a fire's write window never
  laps the read pointer), plus one persistent `int` cursor. Both come from the
  existing table path — `module/tables.rs::ensure_wrtbl_table` already declares a
  per-instance writable table and its state; `builder.rs::{load_table,
  store_table}` are the read-modify-write ops. No new FIR node kind is needed.

- **Lowering — two rates, mirroring the P3 "state advances in fire time" rule
  (`module/clocked.rs`).** Let `p` be the persistent cursor (the sample write
  index).
  - *Inside the fire guard* (once per hop; emitted in the guarded region exactly
    like `PermVar` payloads): a `SimpleForLoop n = 0..N` doing a scatter-**add**
    ```
    fOla[(p + n) mod L] += lane[n]        // load_table + BinOp(Add) + store_table
    ```
    This is the whole O(N) cost, now paid **once per hop**.
  - *Every sample* (top rate, outside the guard): read-and-clear at the cursor
    ```
    out = fOla[p mod L]; fOla[p mod L] = 0; p = p + 1     // O(1)
    ```
    `p mod L` reuses the same masked-index helper the circular delay lines already
    use (`module/state.rs`). Emitting the read-clear at the sample end and the
    scatter-add inside the guard is precisely the co-existence pattern P3 already
    validates for per-domain IOTA / held payloads, so it slots into the existing
    `ClockedState` region redirection rather than being a new scheduling regime.

- **`serialize_in` symmetry (secondary).** `serialize_in(N) = _ <: par(i,N,
  @(N-1-i))` is already O(N)/**hop** in principle (one input delay line, N taps
  read at fire inside the block), but verify it is not being materialized at top
  rate; if it is, the same primitive can expose the window as N reads of one
  shared buffer indexed at fire.

**Validation.** Bit-parity with the current library `serialize_out`: the
`ondemand_pipeline.rs` interleave identities (`interleave(N,id)==@(N-1)`,
`interleave(2,id)==mem`, the frame-delay composition) and the
`ondemand_stft_cola_016` COLA anchor (constant → 1.0) must reproduce exactly;
the impulse-runner effect checks unchanged. Then re-run the §6 benchmark — the
identity-FX `interleave_hop` floor should drop from ~1.75 µs/sample toward the
amortized FFT cost (~0.4 µs).

**Scope / risk.** ~one new box tag + signal node + a lowering module
(≈ the size of the `ondemand` lowering), all reusing the table + fire-time
machinery. The honest cost is the **purity break**: `interleave.lib` stops being
"pure P3-`ondemand` sugar" and gains a second backing primitive. That is the same
trade the FFT-as-a-node fallback makes; both are the price of production-grade
runtime for spectral work.

#### Generality: these are the blocking operators of multirate, not FFT sugar

The FFT is only one payload. `interleave` is the generic **blocking / deblocking**
kernel — `serialize_in` (gather) → `ondemand` (frame-rate compute) →
`serialize_out` (scatter + overlap-add) — shared by every frame-based multirate
structure: analysis/synthesis filter banks, block / partitioned convolution,
per-frame feature extraction (RMS, pitch, MFCC, **neural-net frame inputs**),
LPC. All of them carry the *same* O(N)/sample `serialize_out` cost, so Phase C is
a **general multirate optimization**, worth doing on its own merits independent
of the FFT.

**Measured gather/scatter asymmetry** (N=1024, `clang -O3`, mono @48 kHz):

| stage | pattern | cost | expressible efficiently in pure Faust? |
|---|---|---|---|
| `serialize_in` + `ondemand(sum)` (no scatter) | gather: read N contiguous history samples at fire | **4.2 ns/sample** | **yes** — one delay line + fire-time reads → already O(1)/sample + O(N)/hop |
| `serialize_out` (scatter + OLA) | write N values into an accumulator that overlaps future output | **1751 ns/sample** | **no** — no shared scatter-add in the diagram → O(N)/sample |

So the two sides are **not symmetric**, and the priority is
**`serialize_out` ≫ `serialize_in`**:

- **`serialize_out` as a primitive is the fundamental fix** (the missing O(1)
  scatter-add). It is the whole runtime win and it helps *all* frame-based
  multirate.
- **`serialize_in` as a primitive is optional** for the contiguous case (the
  library form is already ~free). It earns its keep only for (a) **polyphase /
  strided** gather — contiguous `@(N-1-i)` cannot spell the decimated `@(i)`
  a polyphase bank needs — (b) hard-guaranteeing the fire-time lowering against
  future scheduler changes, and (c) symmetry. Lower priority; land it with the
  polyphase use case, not before.

**Are `serialize_in`/`serialize_out` sufficient to "optimize `ondemand` for
multirate" in general? Yes for the *blocking* axis, with three caveats:**

1. They optimize the **glue** (S/P ↔ P/S conversion), not the **payload** — the
   block body still needs Phase A (CSE), and Phase D (rfft / `-vec`) for the FFT.
2. They do **not change rate**: `interleave` is 1-in/1-out at the input rate.
   True rate conversion (output rate ≠ input rate) is the **orthogonal** axis
   already covered by the P3 `upsampling` / `downsampling` primitives.
3. Contiguous-only until `serialize_in` gains a strided variant.

Architecturally: in the polyphase framework, {up-sampler, down-sampler, S/P–P/S
blocking operators, delays} generate all multirate LTI systems. faust-rs already
has `upsampling`/`downsampling` (the **rate-change** axis); `serialize_out`
(+ optionally `serialize_in`) are the missing **blocking** operators. Together
with `ondemand` they complete an efficient multirate primitive set — but each
axis is needed: `serialize_*` alone optimizes blocking, not rate change, not the
payload.

### Phase D — constant-factor runtime wins on the FFT (after A/C)

- **Real-FFT**: the window taps are real; `(_,0)` doubles the work. An `rfft`
  core halves arithmetic and exposes only N/2+1 bins.
- **Twiddle folding**: snap `cos/sin` of rational angles so exact 0 / ±1 / ±j
  are recognized (the code currently carries `6e-17` residues and `×1`); drop
  the dead multiplies.
- **`-vec` / SIMD** across bins once the in-block graph is O(N log N) (the
  STFT `ondemand` is the D1 "scalar island").

### Non-goals / recorded caveats

- True duration-changing time-stretch stays out of the synchronous 1:1 model
  (see the comparison doc). Unrelated to this fix.
- If, after A, a residual super-linear factor remains, revisit whether the
  clocked-block lowering itself duplicates statements per output bin (it should
  not, but verify with the temp-count regression).

## 6. Runtime cost — measured, and why the OLA dominates

Benchmarked the `N=1024` Wiener denoiser
(`tests/corpus/ondemand_stft_denoiser_1024.dsp`) as generated C++, `clang -O3`,
mono @48 kHz, against a hand-written classical STFT denoiser (iterative radix-2
FFT + ring-buffer OLA) on the same machine:

| implementation | ns/sample | % of one core | voices/core |
|---|---|---|---|
| **Faust ondemand N=1024** | **2136** | **10.3 %** | ~10 |
|   ↳ `interleave_hop` harness only (identity FX, no FFT) | 1751 | 8.4 % | — |
|   ↳ FFT + IFFT + gate (amortized to frame rate) | ~385 | ~1.9 % | — |
| classical radix-2 STFT (O(1) ring-buffer OLA) | 93.6 | 0.45 % | ~220 |
| FFTW-based (rfft + SIMD, estimated) | ~20–40 | ~0.1–0.2 % | ~500–1000 |

**Faust is ~23× slower than a naive hand-written STFT, ~50–100× vs FFTW.** The
isolation measurement is the key finding: **82 % of the cost is the
`serialize_out` OLA (O(N)/sample), not the FFT.** The amortized transform itself
(~385 ns) is already ~4× the *entire* classical pipeline (93.6 ns), from the
fully-unrolled 82 k-line `compute()` (I-cache thrash), complex-of-real (no rfft),
and no SIMD/cache-blocking.

Reading: Phase A made `N=1024` *compile* (O(N log N) code); the runtime is still
prototype-grade (~10 voices/core). Phase C (O(1) OLA) then Phase D (rfft, twiddle
folding, `-vec`) are what would move it toward production. Even fully optimized,
the pure-Faust value proposition stays "composable + differentiable + multi-
backend in one graph", not "beats FFTW".

## 7. One-line summary

FFT-in-Faust is not slow because Faust unrolls — it is slow because the FIR CSE
pass does not descend into `ondemand` guarded blocks, so a fully-shared
O(N log N) butterfly DAG is emitted as O(N^2.6) inlined trees. Extending CSE to
run per nested scope is the fix, and it improves compile time and runtime at
once.
