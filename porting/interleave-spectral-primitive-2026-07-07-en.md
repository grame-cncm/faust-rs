# The `interleave` spectral primitive — semantics and phase convention (S1)

Date: 2026-07-07
Status: **locked** — the N=2 unrolled table below and the runtime tests in
`crates/compiler/tests/ondemand_pipeline.rs` (the `interleave_*` cases) fix
the phase convention. Nothing in the spectral track (S3+) may deviate from
it without re-deriving this table.

Tracking surface:
[ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md](ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md)
§4 (analysis) and §7 (S1–S5). This note is the S1 deliverable; the library
definitions (S2) live in `interleave.lib` at the repo root.

## 1. What it is

`interleave(N, FX)` runs a **frame-rate** operator `FX` (one transform per
hop of `N` samples, O(N log N) *per frame*) instead of the *sliding* regime
(per-sample recomputation, O(N log N) *per sample*) that `analyzers.lib`
already offers. It is the clocking harness that makes STFT / phase-vocoder /
differentiable spectral loss expressible in pure Faust. The DSP core (e.g.
`an.fftb`) is reused unchanged; only the *time ↔ frame-width* serialization is
new, and it is built **entirely** on the P3 boolean-`ondemand` block — no new
compiler primitive.

## 2. The decomposition (three stages, only one non-trivial)

```
interleave(N, FX) = serialize_in(N) : periodic_od(FX) : serialize_out(N)
```

with a boolean periodic clock `H` of **period N and phase N−1** (fires at
instants `t ≡ N−1 (mod N)`, i.e. right when the window has just been filled):

```faust
frame_clock(N) = ((+(1) : %(N)) ~ _) == 0;   // counter 1,2,…,N-1,0 ; fire on 0
serialize_in(N) = _ <: par(i, N, @(N-1-i));  // 1 → N : the contiguous window
up0(H, y)       = y * (H != 0);              // zero-stuff = hold masked by clock (option A)
serialize_out(N, H) = par(j, N, up0(H) : @(j)) :> _;  // N → 1 : temporal demux + OLA sum
interleave(N, FX) = serialize_in(N)
                  : ((frame_clock(N), _) : ondemand(FX))    // wait — see note below
                  : serialize_out(N, frame_clock(N));
```

Design points (from the analysis §4.2, verified here):

1. **`serialize_in` splits, it does not fan** — `_ <: par(i, N, @(N-1-i))`.
   A first draft wrote `par(i, N, _@(N-1-i))`, which is N-in→N-out (N
   independent lanes) and makes `interleave` an N-input block. The split
   `<:` is what turns one stream into the N contiguous taps. It operates
   **outside** the `od`, at full rate: inside the decimated domain `@1`
   would mean one tick = N samples and the window would no longer be
   contiguous.
2. **`up0` is option A** — for a boolean periodic clock the fire indicator is
   the clock itself, readable at the outer rate, and `Seq(OD, PermVar(…))`
   exposes the fresh value from the fire tick onward. Zero-stuff upsampling
   `↑₀` is therefore a library one-liner: `y * (H != 0)`. No `SigZeroVar`
   node, no pipeline change. Restriction: boolean clock only (the whole STFT
   case). Option B (`SigZeroVar`) stays the recorded fallback for native RAD
   transpose / performance, should option A ever show a real problem.

## 3. The N=2 unrolled table (the locking anchor)

Input `x = x₀, x₁, x₂, …`. Counter `c[t] = (t+1) mod 2`, fire `H = (c == 0)`
so **fire on odd t** (phase N−1 = 1). `serialize_in(2)` exposes
`l0 = x@1 = x[t−1]`, `l1 = x@0 = x[t]`. The `ondemand` holds `(l0, l1)` into
`(p0, p1)` at each fire and holds between fires. `serialize_out(2)` emits
`up0(p0)@0 + up0(p1)@1`.

| t | x  | c | H (fire) | l0=x[t−1] | l1=x[t] | p0 (hold) | p1 (hold) | up0(p0)@0 | up0(p1)@1 | **out** |
|---|----|---|----------|-----------|---------|-----------|-----------|-----------|-----------|---------|
| 0 | x₀ | 1 | 0        | 0         | x₀      | 0         | 0         | 0         | 0         | **0**   |
| 1 | x₁ | 0 | 1        | x₀        | x₁      | x₀        | x₁        | x₀        | 0         | **x₀**  |
| 2 | x₂ | 1 | 0        | x₁        | x₂      | x₀        | x₁        | 0         | x₁        | **x₁**  |
| 3 | x₃ | 0 | 1        | x₂        | x₃      | x₂        | x₃        | x₂        | 0         | **x₂**  |
| 4 | x₄ | 1 | 0        | x₃        | x₄      | x₂        | x₃        | 0         | x₃        | **x₃**  |

`out[t] = x[t−1]` — i.e. **`interleave(2, id) = mem = @(1) = @(N−1)`**. The
`up0(p1)` slot is delayed by `@(1)` so the second half of a frame is emitted
one sample after the first; the sum reconstructs the contiguous stream. The
`2N−1` latency raised in the initial analysis came from the paper's
hold-with-offset `↑` semantics; option A (fresh value at the fire tick via
`Seq`) gives the tighter **constant latency N−1**.

## 4. Locked facts (runtime-verified)

`crates/compiler/tests/ondemand_pipeline.rs`:

- `interleave(N, id) == @(N−1)` for N ∈ {2, 4, 8} (identity up to a delay);
- `interleave(2, id)` is sample-for-sample `mem` (the table above);
- a stateless frame gain commutes: `interleave(N, 2·id) == 2·@(N−1)`;
- an internal frame delay is measured in **frames**:
  `interleave(N, par(i,N,@(1))) == @(2N−1)` — `@(1)` inside the block advances
  on the per-domain clock (P3 slice 3), so one lane-delay shifts the whole
  reconstructed stream by N samples. This is the composition proof that
  per-domain IOTA and `interleave` cohabit.

## 5. Overlap (hop < N) — for free

The same construction gives overlap-add: use `frame_clock` of **period hop**
(`frame_clock(N, hop) = ((+(1) : %(hop)) ~ _) == 0`), leave `serialize_in`
unchanged (overlapping windows), and the `:>` summation of the delayed
zero-stuffed lines adds the overlapping frames. The COLA condition on the
analysis window stays the user's responsibility (a provable theorem, out of
compiler scope).

## 6. S3 — the framed FFT milestone (done)

`crates/compiler/tests/interleave_fft.rs` (skips gracefully without
`analyzers.lib`). The **analysis-only** framed FFT — a frame-rate FFT whose
O(N log N) butterflies run once per hop, held between frames:

```faust
serialize_in(N) = _ <: par(i, N, @(N-1-i));
fftFX(N) = par(i, N, (_, 0))            // complexify the N real window taps
         : an.c_bit_reverse_shuffle(N)  // reused spatial core, unchanged
         : an.fftb(N);
fft_framed(N) = serialize_in(N) : (frame_clock(N), si.bus(N)) : ondemand(fftFX(N));
```

- **Only `an.fftb`/`an.c_bit_reverse_shuffle` are reused — nothing in the DSP
  core changes.** The "one single brick" claim (§4.5) holds: `ondemand` +
  `serialize_in` are the entire delta. The block exposes **2N held bin reals**
  (`[re₀, im₀, re₁, im₁, …]`) at frame rate.
- **Oracle**: a direct DFT of the known window computed in Rust; at each frame
  tick (`t ≡ N−1 mod N`) the held bins equal `Σᵢ wᵢ·e^(−2πi·m·i/N)` for the
  window `{x[t−N+1 … t]}`. Verified for N ∈ {4, 8}; the O(N log N) butterflies
  scale through the pattern matcher unchanged.
- **Analysis-only mode** (no `serialize_out`): for a spectral *loss* you
  consume the held bins directly — resynthesis (phase vocoder) is the only use
  that needs `serialize_out` + OLA. This is also the surface a differentiable
  spectral loss attaches to (S4).
- **Compile-time note** (§4.6 risk): FFT butterfly lowering recurses deeply;
  the test compiles on a 64 MiB worker stack. N=1024–4096 will genuinely
  stress the pattern matcher and code size — stage with measurements before
  claiming it.

## 7. What comes next (S4+)

- **S4** — differentiable STFT: requires FAD Phase B (P5), since a realistic
  spectral loss crosses the boundary (parameters enter through
  `serialize_in`). Magnitude needs the epsilon `sqrt(R²+I²+ε)`. The
  analysis-only framed FFT above is its infrastructure.
- **S5** — performance: the STFT `ondemand` is a scalar island under `-vec`
  (D1) and the ideal D2 candidate (literal factor, stateless `fftb`).
