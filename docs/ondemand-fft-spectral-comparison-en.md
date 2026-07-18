# Frame-rate FFT in Faust via `ondemand`: a comparison with existing spectral environments

Date: 2026-07-09
Status: synthesis note (grounded in measurements from the
`ondemand-vec-fad-synthesis` branch)

Scope: now that an FFT can be expressed in **pure Faust** on top of the boolean
`ondemand` block (the `interleave.lib` S2 primitive, commit `c088ced`, and the
S3 framed-FFT milestone), how does this approach compare with the way spectral
processing is done elsewhere — in compile time, in the runtime cost of the
generated code, and in the range of spectral algorithms it makes expressible?

All numbers below come from the corpus fixtures added on this branch
(`tests/corpus/ondemand_fft_*.dsp`, `tests/corpus/ondemand_stft_*.dsp`),
compiled with `faust-rs --lang interp` / `--dump-cpp` and exercised with
`impulse-runner`.

---

## 1. The one structural fact: Faust unrolls everything

Every other observation follows from a single architectural property. Faust
does **not** call an FFT routine; it *unrolls* the butterflies into a
straight-line signal graph at compile time. Inspecting the generated C++ for the
analysis-only framed FFT at `N=8` makes both consequences visible.

**The transform is amortized to the frame rate.** The butterfly block sits
inside the frame-clock guard:

```c
for (int i0 = 0; i0 < count; ++i0) {
    int iRecCur320 = ((iRec320 + 1) % 8);
    if (((iRecCur320 == 0) != 0)) {          // fires once every N=8 samples
        fPerm0 = (((float)(input0[i0])) + (fVec17[4] + (fVec17[2] + …)));
        …                                    // the whole O(N log N) transform
    }
    …
}
```

The native *sliding* FFT (`an.rtocv(N) : an.fft(N)`) has **no such guard** — it
recomputes the entire transform on **every sample**. Frame-rate execution is the
headline win of the `ondemand` construction, and before it the sliding regime
was the only way to get an FFT in pure Faust.

**Twiddle factors are folded to immediate constants.** The butterfly bodies
contain literals like `0.7071067690849304f` and `0.00000000000000012246…f`:
there is no twiddle table, no runtime bit-reversal, no loop, no indexing inside
the transform. The transform is pure flat arithmetic the C compiler can schedule
and vectorize.

This dual nature — *amortized + fully unrolled + constant-folded* — is exactly
why the approach is excellent at small sizes on runtime and terrible at large
sizes on compile time.

---

## 2. Compile time

| Environment | FFT compile cost |
|---|---|
| **Faust + `ondemand`** | O(N log N) **unrolled nodes** in the box evaluator. Measured: analysis `N=256` → 9.2 M `.fbc` lines / 8.6 s; **`N=512` fails** (`FRS-EVAL-0099`, eval recursion budget 1024). Full round-trip `N=64` ≈ 12 s, `N=128` > 2 min. |
| Max/MSP `pfft~`, Pd `rfft~`/`ifft~` | ≈ 0 (interpreted patch; FFT is a precompiled library call) |
| SuperCollider `FFT`/`IFFT` + `PV_*` | ≈ 0 (precompiled UGens) |
| Csound `pvsanal`/`pvsynth` | ≈ 0 (precompiled opcodes) |
| C/C++ + FFTW / pffft / KissFFT | < 1 s **regardless of N** (the FFT is a call, not an unrolling) |

Analysis-only scaling ladder (`faust-rs --lang interp`, release):

| N | state | time | `.fbc` lines |
|---|-------|------|--------------|
| 4 | ✅ | 0.13 s | 244 |
| 8 | ✅ | 0.12 s | 770 |
| 16 | ✅ | 0.13 s | 3 818 |
| 32 | ✅ | 0.18 s | 41 289 |
| 64 | ✅ | 0.39 s | 250 665 |
| 128 | ✅ | 1.61 s | 1 519 241 |
| 256 | ✅ | 8.55 s | 9 181 961 |
| **512** | ❌ | — | `FRS-EVAL-0099 stack overflow in eval (depth budget 1024)` |

Two distinct ceilings:

- **Hard ceiling `N=512`**: the box evaluator's guarded recursion budget (1024),
  crossed by the `an.fftb` / `an.c_bit_reverse_shuffle` recursion — *not* FFT
  butterfly codegen.
- **Practical ceiling for round-trips (~`N≤64`)**: a complete
  `fft → effect → ifft` operator plus `serialize_out` roughly doubles the code
  and pushes `N=128` past two minutes, long before the eval-budget wall.

**Verdict.** This is the axis where Faust is by far the worst, and it is *the*
binding constraint. For every other environment the transform size N does not
affect compilation at all; for Faust it dominates it, because "unroll everything
into a signal graph" is the whole compilation model.

---

## 3. Runtime cost of the generated code

| | Faust `ondemand` | FFTW / pffft | Max / SC / Csound |
|---|---|---|---|
| FFT invocations | **1 / hop** (amortized) | 1 / hop | 1 / hop |
| Loop / index overhead | **none** (unrolled) | small | small |
| Twiddles | **immediate constants** | table / registers | table |
| Real-FFT (half the work) | ❌ complex FFT of a real signal | ✅ rfft | ✅ |
| Cache blocking / SIMD | left to the C compiler | ✅ hand-tuned | ✅ |
| CPU load profile | **bursty** (one heavy sample per hop) | same unless smoothed | smoothed by the host buffer |

What this means in practice:

- **At small N (≲ 64)** the unrolled Faust code — immediate twiddles, no loop
  overhead, no runtime bit-reversal — **can match or beat** a generic FFTW call:
  no setup, and the C compiler schedules/vectorizes the flat expression. The
  `if(frame_clock)` guard divides the FFT cost by ~N relative to the sliding
  regime. This is a genuinely competitive operating point.
- **At large N it loses**, for four compounding reasons:
  1. it computes a **complex** FFT of a real input → ~2× the necessary work
     versus a real-FFT (`rfft`);
  2. code size is O(N log N) → I-cache pressure and register spilling make the
     unrolled code **slower** than a compact looped FFTW;
  3. residual twiddles (`6e-17`, …) that a hand-tuned FFT special-cases to
     ×1 / ×j / ×−1 stay as real multiplies → wasted FLOPs;
  4. the whole spectral block lives in a single branch → a **bursty** worst-case
     load (one very heavy sample every hop), which is adverse for real-time
     scheduling headroom.

**Verdict.** Excellent at small N thanks to amortization + constant folding +
zero overhead; below dedicated FFT libraries at large N.

---

## 4. What spectral algorithms become expressible

**Now expressible in pure Faust** (demonstrated by the corpus fixtures on this
branch, each verified numerically with `impulse-runner`):

- **Analysis** — spectral loss / analyzers via the analysis-only framed FFT
  (`il.serialize_in : (il.frame_clock, si.bus) : ondemand(fftFX)`), consuming the
  2N held bin reals directly.
- **Round-trip effects** via `il.interleave(N, FX)` with an N→N frame operator
  `real→complex : an.fft : <effect> : an.ifft : real-part`:
  - brickwall **low- / high- / band-pass** by Hermitian-symmetric bin masks
    (LP impulse response is exactly `(1 + 2·Σcos)/N`);
  - **fast convolution** — per-bin product with a fixed kernel spectrum
    `H(m) = DFT{h}` folded at compile time; rectangular framing yields a
    *circular* convolution per frame (impulse response `0.5 @(N-1) + 0.5 @N` for
    `h = [0.5, 0.5]`);
  - **robotization** (keep magnitude, zero phase).
- **Overlap-add STFT** via `il.interleave_hop(N, hop, FX)`: a periodic Hann
  analysis window at `hop = N/2` satisfies COLA (`wa[n] + wa[n+N/2] == 1`), so an
  identity spectral stage reconstructs the input **exactly** in steady state
  (verified: the reconstruction reaches 1.0; the rectangular-window control gives
  gain 2.0, proving the window — not the harness — is the COLA agent).
- **Phase-vocoder with inter-frame phase accumulation.** A per-bin one-frame
  phase memory (`ph'`) and a synthesis phase accumulator (`psi = incr : +~_`)
  running **once per frame inside the `ondemand` block** — verified in the
  generated C++ to be lowered inside the frame-clock guard, i.e. as frame-rate
  state. The phase-propagation identity reconstructs a sine exactly
  (`max|wet[t] − dry[t−(N-1)]| = 1.5e-5`); a constant per-bin phase rate gives a
  correct single-sideband **frequency shifter** (DC → a pure tone at the shift
  frequency). This is the recursion-inside-the-block shape the other effects
  above do not need but a vocoder does.

**Faust's distinctive upside** — this is where it separates from Max/SC/Csound:

- **One language.** The spectral block composes with the rest of the DSP graph
  with no FFI boundary and no manual buffer juggling.
- **Differentiable.** Gradients flow through the FFT (FAD/RAD) → a
  **differentiable STFT / trainable spectral loss** (the S4 goal, DDSP-style).
  No mainstream real-time environment offers gradient-through-FFT in the same
  graph. This is the strategic payoff of building the FFT on the differentiable
  `ondemand` substrate.
- **Backend portability.** It compiles to every Faust backend (C / C++ / Rust /
  wasm / interpreter) with no external dependency.

**What is still hard / not yet done:**

- **Duration-changing time-stretch** (analysis hop ≠ synthesis hop) is *not*
  expressible in a synchronous 1-in/1-out Faust `process` at constant rate: the
  graph cannot emit more (or fewer) samples than it consumes. It needs an
  external rate-decoupling buffer, outside the synchronous model. Note this is a
  property of the *streaming rate contract*, **not** of the phase machinery — the
  inter-frame phase accumulation itself is now expressible (see above), and its
  constant-rate products (identity reconstruction, frequency shifting) work.
- A **clean pitch-shift** still wants either bin reassignment (a scatter, awkward
  in a static graph) or time-stretch-plus-resample (which reintroduces the rate
  issue). The same open items remain for phase-locked vocoders, transient
  handling, constant-Q / multi-resolution, and adaptive hop.
- Max/SC/Csound ship these **ready-made** (`PV_MagFreeze`, `PV_BinShift`,
  `pvscale`, `pvstanal`, …). Faust now has the **substrate and the inter-frame
  recursion** to build them — uniformly and differentiably — but you build them.

---

## 5. Bottom line

The `ondemand` construction moves the FFT in Faust from the **sliding regime**
(O(N log N) *per sample* — the only option before) to the **frame regime**
(O(N log N) *per hop*), an ~N-fold compute reduction, at the price of a
**compile model that unrolls everything** (hard wall at `N=512`, practical
round-trip ceiling ~`N≤64`).

Against dedicated FFT libraries and the environments built on them, Faust:

- **loses** on compile time (size N dominates it) and on large-N runtime (no
  real-FFT, no cache blocking, code bloat, bursty load);
- **wins**, decisively, by making spectral processing **composable, dependency-
  free, multi-backend, and differentiable** inside a single graph — with small-N
  runtime that is already competitive with a generic library FFT.

The "inter-frame" gap versus SuperCollider / Csound — per-bin state accumulating
across frames — is now closed: a phase-propagation vocoder (exact reconstruction)
and a frequency shifter run with the accumulator lowered as frame-rate state
inside the `ondemand` block. What remains is orthogonal to that machinery: a
*duration-changing* time-stretch is barred by the synchronous rate contract, and
a clean pitch-shift wants bin reassignment or resampling. The natural next step
is instead to wire **FAD Phase B (P5)** through this STFT to reach the S4
differentiable-STFT milestone — for which the phase-propagation identity is
already the infrastructure.

---

## 6. Scalability: the compile/runtime wall is a fixable CSE gap

The "Faust unrolls everything" framing above is only half the story on cost. A
closer measurement shows the generated in-`ondemand` FFT is **O(N^2.6)**, not
O(N log N): arithmetic op count grows ~6× per doubling of N (`ops/(N log N)`
diverges 7.8 → 262 for N=8→128), and the code holds only ~2N temporaries — the
butterfly sharing is gone.

Crucially this is **not** intrinsic to Faust: the *plain* `an.fft(N)` (outside
`ondemand`) is O(N log N) with a flat `ops/(N log N)` (8.0 → 9.0) and O(N log N)
temporaries. Signals and FIR nodes are both hash-consed (the shared DAG exists);
a correct FIR CSE pass materializes it — **but only for the flat statement
buckets.** It deliberately does not descend into nested block bodies, and the
`ondemand` guarded block is exactly such a body, so the fully-shared butterfly
DAG is emitted as 2N inlined trees.

The fix — running CSE **per nested scope** — is localized and improves *both*
compile time and runtime at once (O(N^2.6) → O(N log N)), which is what would
make FFT-in-Faust genuinely usable at large N. Full diagnosis and phased plan:
[`porting/fft-scalability-cse-in-clocked-blocks-2026-07-09-en.md`](../porting/fft-scalability-cse-in-clocked-blocks-2026-07-09-en.md).

---

### References

- Semantics and phase convention:
  `porting/interleave-spectral-primitive-2026-07-07-en.md`.
- Roadmap: `porting/ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md` §7.
- Runtime lock: `crates/compiler/tests/interleave_fft.rs`,
  `crates/compiler/tests/ondemand_pipeline.rs`.
- Corpus examples: `tests/corpus/ondemand_fft_*.dsp`,
  `tests/corpus/ondemand_stft_*.dsp`.
- Session log: `porting/journal/2026-07-09.md`.
