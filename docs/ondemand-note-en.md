---
title: "Note: the ondemand / upsampling / downsampling primitives in faust-rs"
author: "Claude Opus 4.8"
date: "2026-07-21"
---

# Clock domains in `faust-rs`: `ondemand`, `upsampling`, `downsampling`

French version: [ondemand-note-fr.md](ondemand-note-fr.md) (same content; keep
both in sync on amendment).

This note presents the three clock-domain primitives from the Faust
programmer's point of view. It assumes no knowledge of the compiler internals.

In ordinary Faust, every signal advances at one rate: one sample per tick, for
the whole program. The clock-domain primitives break that assumption. They let a
sub-expression run **at its own rate** — less often, more often, or only when
something happens — while the rest of the program keeps running at the audio
rate.

```faust
ondemand(C)
upsampling(C)
downsampling(C)
```

## 1. The clock is an extra input

All three take one expression and return a *wrapped* expression with **one more
input than the body**:

> if `C : u → v` then `ondemand(C) : u+1 → v`

That extra **first** input is the clock `H`. It is an ordinary Faust signal —
you can compute it, gate it, derive it from a UI element:

```faust
// The body runs only while the button is held.
process = (button("gate"), _) : ondemand(*(2));
```

The same arity rule applies to `upsampling` and `downsampling`.

## 2. What each primitive does with the clock

| Primitive | Clock reading | Effect per outer tick |
|---|---|---|
| `ondemand`, clock range ⊆ [0,1] | run if `H ≠ 0` | body executes 0 or 1 time |
| `ondemand`, clock range wider than [0,1] | run `H` times | body executes `H` times |
| `upsampling` | `H` = factor | body executes `H` times |
| `downsampling` | `H` = period | body executes on every `H`-th tick |

The two `ondemand` rows are not two primitives, and the choice is not something
you declare: the compiler infers it from the clock signal's **value range**.

- If the inferred range is contained in **[0,1]**, the clock is read as a
  condition and the body runs at most once, whenever `H ≠ 0`.
- If the range is wider, the clock is read as a **count** and the body runs `H`
  times.

Note what the first case does *not* say: it is the range that must lie in
[0,1], not the individual values. A clock that can take the value `0.5` still
has range ⊆ [0,1], so it is a condition — and since `0.5 ≠ 0`, the body runs
**once**, not half a time. There is no fractional execution; if you want a
count, give the clock a range that exceeds 1.

Constant clocks are simplified away early:

- `H == 0` — the body never runs and the outputs are replaced by `0`;
- a constant non-zero clock collapses to the corresponding fixed structure.

So `ondemand` with a literal clock costs nothing at run time; the primitives are
only "dynamic" when the clock is.

## 2 bis. `ma.SR` inside a domain

`upsampling` and `downsampling` change the *rate* of the body, so they also
change what the body means by "the sample rate". `ma.SR` is adapted
automatically:

| Context | Value of `ma.SR` inside the body |
|---|---|
| `upsampling(C)` with clock `H` | `SR * H` |
| `downsampling(C)` with clock `H` | `SR / H` |
| nested US/DS | the factors compose |
| `ondemand(C)` | **unchanged** — still the outer `SR` |

Verified on emitted C++: under `upsampling` with clock 2 the constant becomes
`fSampleRate * 2`; under `downsampling` with clock 4 it becomes
`fSampleRate * 0.25`; and `upsampling(2)` wrapping `downsampling(4)` yields
`fSampleRate * 0.5`, i.e. the whole stack of factors is unrolled.

This is what you want most of the time: a filter whose coefficients are computed
from `ma.SR` inside an `upsampling` body is *automatically* tuned for the
oversampled rate, with nothing to pass in by hand.

**The `ondemand` row is the trap.** `ondemand` does not adapt `ma.SR`, and it
cannot: its firing rate depends on the clock signal at run time, so there is no
constant ratio to fold into `SR`. A body that computes coefficients from
`ma.SR` inside an `ondemand` will therefore be tuned for the *outer* rate, not
for its own firing rate. If your body needs its effective rate, compute it
outside and pass it in as an ordinary input.

## 3. The subtlety that matters: time inside a domain is local

This is the part that surprises people, and it is the whole point of the
construct.

Inside a clock domain, **time advances at the domain's rate, not the audio
rate**. A one-sample delay inside an `ondemand` body is one *firing* late, not
one audio sample late. The same holds for every stateful construct: delay lines,
recursion (`~`), tables, and accumulators all count in *fire time*.

```faust
// `prev` is the previous value produced *while the gate was open*,
// not the value one audio sample ago.
process = (button("gate"), _) : ondemand(+ ~ _);
```

If you want audio-rate history, keep the state outside the domain and pass it in.
If you want per-event history — a counter of events, the previous frame, a value
held between firings — put it inside. Choosing the wrong side is the most common
source of confusion, and it is silent: both versions compile.

"Inside" is decided by the text, not by the value. A *definition* referenced
in the body (`ba.time`, an oscillator, a table read, a delayed signal, a
`with` local) is instantiated again in the body's domain, with its own state
advancing in fire time, even when the same definition is also used outside:
the compiler annotates every stateful primitive built inside a domain with
that domain, so the two instances never share their state. What reaches the
body from outside as a *value* — a block input, a lambda parameter bound
outside — is the outer signal sampled at the firing.

```faust
t = ba.time;
process = (clock, t) : ondemand(\(u).(u, float(t)));
// u       : the audio-rate time, sampled at each firing
// float(t): a fresh counter of firings
```

## 4. Typical use cases

**Control-rate computation.** Anything that does not need to be recomputed 48000
times a second: envelope followers feeding a display, parameter smoothing logic,
expensive analysis. Wrap it in `downsampling` and pick a period.

```faust
process = (256, _) : downsampling(expensive_analysis);
```

**Event-triggered work.** A body that should run only when something happens —
a note-on, a threshold crossing, a button. `ondemand` with a 0/1 clock is
exactly this, and unlike a `select2` it does not *compute both branches*: the
body genuinely does not execute.

**Oversampling a nonlinearity.** Run a saturator or an oscillator at a multiple
of the audio rate to push aliasing up, using `upsampling`. Note that the
primitive controls *execution rate* — the anti-aliasing filters around it are
still yours to write.

**Frame-rate / spectral processing.** Combined with the `interleave.lib`
primitive `il.interleave(N, FX)`, `ondemand` lets a frame operator run once per
`N` samples, which makes per-frame FFT and STFT-style processing expressible in
plain Faust. `il.interleave(N, id)` is exactly `@(N-1)`, so the round-trip
latency of the construct is `N-1` samples. See
[ondemand-fft-spectral-comparison-en.md](ondemand-fft-spectral-comparison-en.md).

## 5. Spectral processing with `ondemand` and `interleave.lib`

### From an audio stream to a frame operator

The FFT operators in `analyzers.lib` are spatial circuits: an `N`-point
transform expects the `N` samples of a frame at the same logical instant.
`libraries/interleave.lib` provides the streaming harness that connects such a
frame operator to an ordinary one-sample-at-a-time Faust stream:

```text
audio stream
  -> serialize_in(N)
  -> frame_clock(N) + ondemand(FX)
  -> serialize_out(N)
  -> audio stream
```

`serialize_in(N)` exposes the latest `N` samples as `N` parallel lanes. The
boolean frame clock fires when the window is complete, so `ondemand(FX)` runs
the FFT or the complete spectral effect only once per frame. `serialize_out`
holds, delays, and sums the result lanes back into a stream. The convenience
operator `il.interleave(N, FX)` packages this non-overlapping path; use
`il.interleave_hop(N, hop, FX)` for overlapping windows and overlap-add.

For analysis without resynthesis, the bins can remain as held frame-rate
signals:

```faust
il = library("interleave.lib");
an = library("analyzers.lib");
si = library("signals.lib");

N = 128;
fftFX(NN) =
    par(i, NN, (_, 0))
    : an.c_bit_reverse_shuffle(NN)
    : an.fftb(NN);

process =
    il.serialize_in(N)
    : (il.frame_clock(N), si.bus(N))
    : ondemand(fftFX(N));
```

For a time-to-spectrum-to-time effect, `FX` complexifies the frame, transforms
it, modifies the complex bins, and performs the inverse transform. This compact
example implements a 16-point brick-wall low-pass:

```faust
il = library("interleave.lib");
an = library("analyzers.lib");

N = 16;
kc = 4;
gain(m) = float(min(m, N-m) <= kc);
scaleBin(g) = *(g), *(g);
mask = par(m, N, scaleBin(gain(m)));

lowpass(NN) =
    par(i, NN, (_, 0))
    : an.fft(NN)
    : mask
    : an.ifft(NN)
    : par(i, NN, (_, !));

process = il.interleave(N, lowpass(N));
```

Compile examples that import the project-local library with `-I libraries`, in
addition to the path containing the standard Faust libraries.

### Checked DSP examples

The corpus contains complete programs that can be used as starting points:

- [ondemand_fft_framed_128.dsp](../tests/corpus/ondemand_fft_framed_128.dsp)
  exposes held FFT bins for analyzers or spectral losses;
- [ondemand_fft_roundtrip_id_016.dsp](../tests/corpus/ondemand_fft_roundtrip_id_016.dsp)
  checks exact non-overlapping FFT/IFFT reconstruction after the `N-1` latency;
- [ondemand_fft_lowpass_016.dsp](../tests/corpus/ondemand_fft_lowpass_016.dsp),
  [ondemand_fft_highpass_016.dsp](../tests/corpus/ondemand_fft_highpass_016.dsp),
  and [ondemand_fft_bandpass_016.dsp](../tests/corpus/ondemand_fft_bandpass_016.dsp)
  apply Hermitian-symmetric spectral masks;
- [ondemand_fft_fastconv_032.dsp](../tests/corpus/ondemand_fft_fastconv_032.dsp)
  performs per-frame convolution by multiplying complex bins;
- [ondemand_stft_robot_ola_016.dsp](../tests/corpus/ondemand_stft_robot_ola_016.dsp)
  combines a Hann window, 50% overlap, overlap-add, and magnitude-only
  robotization;
- [ondemand_stft_pv_freqshift_016.dsp](../tests/corpus/ondemand_stft_pv_freqshift_016.dsp)
  keeps per-bin phase state inside the frame domain to implement a
  phase-vocoder frequency shifter;
- [ondemand_stft_denoiser_1024.dsp](../tests/corpus/ondemand_stft_denoiser_1024.dsp)
  is a larger Wiener-style spectral gate.

### Compilation and generated-code limits

`ondemand` fixes the most important execution-rate problem: the transform runs
once per hop rather than once per audio sample. It does not, however, turn the
FFT into an opaque backend intrinsic. The current compiler expands
`an.fft(N)`/`an.ifft(N)` into a hash-consed graph of scalar butterfly
expressions before code generation.

This has two practical consequences:

- **Compilation grows with the transform.** Even with common-subexpression
  elimination inside the clocked block, the compiler must build, type,
  transform, and emit an `O(N log N)` graph. Large transforms consume much more
  time and memory than a call to a precompiled FFT library, and the recursive
  Faust FFT definition can require a raised evaluator depth. The checked
  1024-point denoiser uses `FAUST_RS_DEFAULT_EVAL_MAX_DEPTH=4096` and is an
  intentionally heavy example (about 0.5 million interpreter lines and roughly
  40 seconds to compile on the machine used for its validation).
- **The generated transform is scalar and fully specialized.** Small FFTs can
  benefit from constant-folded twiddles, no planning overhead, and no inner
  indexing loop. At larger sizes, code size, instruction-cache pressure,
  register spilling, and the burst of work on a frame tick become limiting.
  The current path also computes a complex FFT for real input and cannot match
  a tuned FFT implementation that uses real-FFT kernels, cache-aware staged
  loops, explicit SIMD, or architecture-specific assembly.

These are limits of the current lowering path, not of the `ondemand` semantics.
A future FFT primitive or backend-level structured FFT IR could retain the
frame-rate behavior while generating a compact looped or library-backed kernel.
For now, small and medium transforms are the natural operating range; measure
both compile time and worst-case frame-tick CPU cost before using a large FFT in
a real-time application.

Finally, framing remains the programmer's responsibility:
`il.interleave(N, FX)` has a fixed `N-1` round-trip latency and implements
non-overlapping rectangular frames. With `interleave_hop`, select analysis and
synthesis windows satisfying the required overlap-add condition. A
duration-changing time stretch still needs rate decoupling outside a
synchronous one-input/one-output Faust process.

## 6. Links with FAD and RAD

Clock domains are the practical vehicle for **in-graph learning** — the
applicative motivation behind much of this machinery. See
[fad-note-en.md](fad-note-en.md) and [rad-usage-en.md](rad-usage-en.md) for the
differentiation primitives themselves.

**Learning at control rate.** A gradient step does not need to run per sample.
Wrapping an optimizer in a domain decouples adaptation rate from audio rate.
With [`optimizers.lib`](../libraries/optimizers.lib) (prefix `op`) and the
frame clock of `interleave.lib`:

```faust
// The whole loop in fire time: one optimizer step every 64 samples, on the
// block's inputs (the excitation and the target sampled at the firing).
learn(xi, ti) = op.descend_1D(\(g).(op.mse(g * xi, ti)), op.adam_g(0.02, 0.9, 0.999, 1e-8), -4, 4, 0, 0);
g = (il.frame_clock(64), x, target) : ondemand(learn);
```

**Event-triggered adaptation.** `ondemand` with a 0/1 clock gives you
"adapt only while this gate is open", which is how you freeze a learned
parameter outside a training phase without adding branches to the audio path.

**Decimated gradients.** Compute a loss at audio rate but update at a lower
rate, keeping the expensive part of the backward pass in a slower domain. This
is what `op.descend_1D_clocked` … `op.descend_5D_clocked` do: the gradient is
computed on every sample, averaged over the frame by `op.frame_mean`, and the
step is taken inside an `ondemand` block, so the engine's state advances once
per frame — a mini-batch step:

```faust
// Gradient at audio rate, one SGD step per 64-sample frame: exact within a
// few frames.
g = op.descend_1D_clocked(il.frame_clock(64), loss, op.sgd_g(0.5), -4, 4, 0, 0);
```

**Frame-rate DDSP.** With `interleave`, a differentiable spectral loss becomes
expressible: FFT the frame, compare against a target spectrum, differentiate the
result. Section 10 of [fad-rad-synthesis-en.md](fad-rad-synthesis-en.md) and
section 11 of
[optimizers-ddsp-tutorial-en.md](../libraries/optimizers-ddsp-tutorial-en.md)
show a gain learned on an 8-point FFT loss, one optimizer step per frame, with
the frame passed to the block as named inputs.

**One rule to remember:** differentiation and clock domains compose *inside* a
domain, but a derivative does not flow **across** a domain boundary. `fad`
inside an `ondemand` body is supported, and its tangents are validated
numerically against finite differences. Differentiating a signal that crosses
into or out of a domain is a different matter: the compiler has a dedicated
diagnostic for automatic differentiation reaching a clock-domain boundary it
cannot cross (`FRS-PROP-0004`). If you are building a learning loop, keep the
seed, the loss, and the update in the **same** domain.

## 7. Practical notes and current limits

- The clock is a signal, so it can itself be computed inside another domain.
  Nesting works, but reason in fire time at each level — the rates multiply.
- `ondemand` with a clock ranging beyond 1 executes the body `H` times per tick. A clock
  derived from an unbounded computation can therefore make one audio tick
  arbitrarily expensive; bound it if it comes from user input.
- Clock domains compose with vector mode (`-vec`); stateful shapes inside a
  domain run in fire time in every backend.
- The C++ Faust compiler is the reference for the clocked machinery itself, but
  **there is no C++ reference for the combination of FAD/RAD with clock
  domains** — faust-rs defines those semantics, and the oracle is numerical
  agreement with finite differences.

A definition referenced inside a body is re-instantiated in the body's
domain (section 3): `ba.time` inside a block counts firings, and the same
`x @ n` written inside and outside a block are two delay lines. A block
input, or a lambda parameter bound outside, is sampled at the firing. Table
generators are the exception by nature: a table is filled once, outside
every domain, wherever it is read.

A `rad` whose loss and seeds live inside a body runs in the block's domain,
at frame rate: the harmonic synthesizer of
[libraries/ddsp-examples-en.md](../libraries/ddsp-examples-en.md) (example
11) fits sixteen amplitudes through a spectral loss computed once per frame
that way.

## See also

- [fad-note-en.md](fad-note-en.md) — forward-mode differentiation
- [rad-usage-en.md](rad-usage-en.md) — reverse-mode differentiation
- [ondemand-fft-spectral-comparison-en.md](ondemand-fft-spectral-comparison-en.md) — spectral processing built on these primitives
- `porting/ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md` — compiler-side semantics and port plan
- `porting/ondemand-fad-rad-cohabitation-2026-06-10-en.md` — FAD/RAD × domains, in detail
