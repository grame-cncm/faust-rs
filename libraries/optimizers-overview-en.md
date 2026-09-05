# `optimizers.lib`: an overview

French version: [optimizers-overview-fr.md](optimizers-overview-fr.md) (same
content; keep both versions synchronized). A hands-on companion is the
tutorial [optimizers-ddsp-tutorial-en.md](optimizers-ddsp-tutorial-en.md).

This document is written for someone who knows Faust but has never trained
anything: no machine learning, no automatic differentiation, no
"differentiable DSP" background is assumed. It explains what the library is
for, why the `fad`/`rad` primitives it rests on are worth attention in the
field of differentiable DSP, how the library file is organized, and where each
algorithm comes from and why it was chosen. Every number quoted was measured
on the current compiler with `faustprobe`; the programs are in the tutorial.

## 1. Three ideas

**Learning.** A DSP has parameters: a gain, a cutoff, five biquad
coefficients. Usually a human sets them. *Learning* means letting the program
set them itself, by comparing what it produces with what it should produce and
moving the parameters in the direction that shrinks the difference. That
difference, reduced to one number, is the **loss**: for example the squared
error `(model - target)^2` at the current sample.

**Gradient.** To move a parameter in the right direction, one needs to know
how the loss changes when the parameter changes: the derivative of the loss
with respect to the parameter. For several parameters, the vector of these
derivatives is the **gradient**. *Gradient descent* is the update
`p <- p - lr * gradient`, repeated: `lr`, the learning rate, sets the step
size. Everything in this library is a refinement of that one line.

**Automatic differentiation.** Writing derivatives by hand is error-prone
and does not survive a change to the model. Automatic differentiation (AD)
computes them mechanically from the program that computes the value, by
applying the chain rule to each operation. It is exact (not a finite
difference) and it costs a small constant factor over the original
computation. Deep learning is built on it; so is *differentiable DSP* (DDSP):
signal-processing code whose parameters are trained by gradient descent,
introduced to the audio world by Engel et al. in 2020.

## 2. What is new about `fad` and `rad` in Faust

### 2.1 How DDSP is usually done

The common practice is to rewrite the DSP in a machine-learning framework
(PyTorch, TensorFlow, JAX): the oscillator, the filter, the reverb become
tensor operations, the framework differentiates them, training runs offline
on batches of audio in Python, and the learned parameters are then exported to
a separate real-time implementation. This works, and it is how most published
DDSP results were obtained, but it has costs that the audio practitioner feels
directly:

- the DSP exists twice, once for training and once for deployment, and the
  two must be kept equivalent;
- recursive filters (IIR, feedback, anything with a `~`) are awkward: a
  per-sample loop is slow in a tensor framework, so filters are approximated,
  truncated, or given dedicated kernels;
- learning happens in a training script, not in the instrument or the
  effect: an adaptive plugin that keeps learning on stage is out of reach.

### 2.2 What the Faust primitives do instead

`faust-rs` adds two primitives to the language:

```faust
fad(expr, seeds)   // forward mode: primal outputs followed by their tangents
rad(expr, seeds)   // reverse mode: primal outputs followed by gradients
```

The derivative is computed **by the compiler, at compile time, on the signal
graph itself**. `fad(expr, p)` is not a runtime graph or a callback into a
framework: it is expanded during propagation into ordinary Faust signals, then
compiled to C++, Rust, WebAssembly, or the interpreter like any other signal.
Three consequences matter for DDSP:

1. **One program.** The model, its derivative, the loss, and the optimizer
   are all Faust code in the same file. What is trained is what is deployed;
   there is no export step and no second implementation.
2. **Recursion is differentiated exactly.** A feedback loop is differentiated
   by *augmenting its state*: the recursion carries `[value, derivative]`
   instead of `value` alone, so the derivative of a recursive filter with
   respect to a coefficient is the exact causal derivative, sample by sample,
   with no truncated window. In machine-learning terms this is real-time
   recurrent learning (RTRL, Williams & Zipser 1989) obtained for free from
   the structure of the program.
3. **Learning runs where the audio runs.** The update `p <- p - lr * g` is a
   Faust recursion like any other, so an effect can keep adapting inside a
   plugin, a browser page, or an embedded target, one sample at a time, with
   the same real-time guarantees as the rest of the DSP.

The idea has a lineage: dual numbers (Clifford, 1873), forward-mode AD
(Wengert, 1964), reverse-mode AD and backpropagation (Linnainmaa, 1976;
Rumelhart et al., 1986). `faust-rs` brings it into the language with explicit
seeds (`fad(expr, seeds)`: any signal can be a seed, several at once), covers
recursion, tables and clock-domain blocks in forward mode, and implements
reverse mode with a block-local backward sweep for outputs handed to a host
and a one-sample sweep for a gradient consumed inside the graph. The
technical notes are
[docs/fad-note-en.md](../docs/fad-note-en.md) and
[docs/rad-note-en.md](../docs/rad-note-en.md).

### 2.3 Forward or reverse

| | `fad` (forward) | `rad` (reverse) |
|---|---|---|
| Output | each primal followed by one tangent per seed | all primals, then one gradient per seed |
| Cost grows with | the number of seeds (parameters) | the number of outputs: one sweep gives every gradient |
| Through recursion | exact causal derivative (augmented state) | as a public output: block-local sweep over the current `compute` block, zero terminal adjoint; consumed inside the graph: the sample itself, the past state held fixed (the direct term) |
| Consumable in the graph | yes, the natural choice for an in-graph optimizer | yes: the same gradient for a feed-forward model, the pseudo-linear-regression gradient through a recursion |
| Typical use | learning loops in Faust, Newton solvers, local slopes | many parameters under one loss (the bus loops), gradients handed to a host that accumulates over a block |

For a handful of interpretable parameters — which is what an audio model
usually has — forward mode is the right tool for a loop written in Faust, and
it is what the fixed-arity loops of `optimizers.lib` use. Reverse mode is the
economical direction when one scalar loss depends on many parameters: one
sweep gives them all, where forward mode carries one tangent per parameter.
The bus loops of the library exist in both modes; on a 16-tap FIR the `rad`
version compiles to 3x fewer interpreter instructions and runs 2.5x faster,
7x and 10x at 64 taps, for the same trajectory. The price, inside a loop, is
the horizon: the gradient is consumed at the sample that produces it, so the
reverse sweep sees that sample only, and through a recursion of the model it
returns the direct term — the past state held fixed — where `fad` carries the
exact derivative (section 4.7). For a feed-forward model (an FIR, a gain, a
waveshaper) the two are the same number. Handed to a host as a public output,
`rad` works block by block; see
[docs/rad-usage-en.md](../docs/rad-usage-en.md).

### 2.4 Clock domains: `ondemand` and learning at its own rate

Faust runs every signal once per sample. `faust-rs` adds three primitives
that let a sub-expression run **at its own rate**: `ondemand(C)`,
`upsampling(C)` and `downsampling(C)`. Each takes a body `C` and returns it
with one more input, a *clock*: `ondemand(C)` runs `C` only on the samples
where the clock is non-zero and **holds** its outputs in between; inside the
body, time is *fire time* — a `~` recursion or a delay counts firings, not
audio samples. The note [docs/ondemand-note-en.md](../docs/ondemand-note-en.md)
is the reference; what matters here is the partnership with `fad`.

A learning loop has two rates in it that nothing forces to be equal: the
audio rate, at which the model must run, and the *adaptation* rate, at which
the parameters move. Training frameworks move parameters once per batch;
adaptive filters move them once per sample; an `ondemand` block lets a Faust
program choose, and the derivative machinery composes with it:

- **`fad` inside a block** is supported and checked against finite
  differences by the compiler tests: the primal and its tangents are computed
  together at each firing and held together in between. The block's inputs
  (a frame of `N` samples, say) are zero-tangent signals; a seed that lives in
  the body, or enters it as an explicit input, is differentiated normally.
- **`fad` around a block** differentiates through the held output.
- **A derivative never crosses a clock boundary on its own.** The clock is
  opaque to differentiation, `rad` across a domain boundary is rejected, and
  the rule of thumb of the ondemand note is the one to follow: keep the seed,
  the loss and the update in the same domain, and pass whatever the body needs
  from outside as an *explicit* input of the block.

Three patterns follow, all measured (the programs are in tutorial section 11):

| Pattern | Where things run | Result |
|---|---|---|
| **Learning at control rate**: the whole `descend_1D` loop inside `ondemand`, clocked every 64 samples | model, loss, `fad` and update all in fire time; the parameter is held between firings | gain 0 → 0.7000 with 312 optimizer steps in 20 000 samples; the `fad` graph runs 64 times less often |
| **Audio-rate gradient, frame-rate update**: `fad` and an `ema` of the gradient at audio rate, the parameter step in a block that receives the previous value and the averaged gradient as inputs, the held parameter fed back with `~` | gradient at audio rate, update at frame rate | gain exact (`0.700000`) after 4 000 samples: averaging the gradient over the frame loses nothing |
| **Frame-rate spectral loss**: `interleave.lib` serializes the audio into frames of `N` samples, and `descend_1D` runs inside the block on a loss computed by `an.fft` on the frame — one optimizer step per frame | everything in the frame domain; this is the DDSP spectral loss, in Faust | a gain scaling an 8-point spectrum settles at 0.34, the least-squares optimum for the excitation being 0.340 |

The third pattern is the one that brings Faust closest to how DDSP papers
train: a loss on a spectrum rather than on a waveform, updated per frame.
Its variant with the parameter kept outside the block and passed in as an
explicit input (the block then outputs the held gradient and the update is
gated by the clock) gives the same 0.34.

The second pattern is what the library packages as `descend_1D_clocked` …
`descend_5D_clocked`: the loss and `fad` at audio rate, the gradient averaged
over the frame by `frame_mean` (an exact mean, reset by the clock), the step
taken inside an `ondemand` block so the engine's moments and schedules advance
once per frame, the parameter held at `init` until the first firing and a
reset shorter than a frame latched until the next one. Measured: a gain exact
(`0.700000`) after 4 000 samples with one SGD step of 0.5 per 64-sample frame;
`(log f, q)` of a resonant filter with Adam per frame at
`(1200.2, 1.996)` after 10 000 samples, then within 1 % of `(1200, 2.0)`.

Two practical rules. A frame operator written with free
`_` inputs must not be passed around as an open expression — every use
duplicates its inputs, and the block's arity explodes; give the body named
arguments instead (`learn(x0, ..., x7)`). And `ma.SR` is *not* adapted inside
`ondemand` (its rate is not known statically), so anything that depends on
the block's own rate must be computed outside and passed in.

### 2.5 What it is not

`fad`/`rad` do not turn Faust into a deep-learning framework, and this library
does not pretend otherwise:

- seeds are explicit: you name what you differentiate with respect to;
- some nodes have no derivative rule and give a **zero tangent** silently:
  buttons, checkboxes, integer arithmetic and comparisons, integer casts,
  table writes. A learned parameter must not pass through them inside the
  model;
- `abs` is differentiated as `x / abs(x)`, which is `NaN` at zero — the
  library's losses avoid it;
- there is no batching and no data loader: learning is online, one sample at
  a time, on the audio that flows through the program.

## 3. How the library is organized

The file [optimizers.lib](optimizers.lib) (prefix `op`, version 0.7.0) is
documented function by function in the Faust libraries convention; this
section gives the map. It has twelve sections, ordered from building blocks to
ready-made loops.

| Section | What it holds | Why it exists |
|---|---|---|
| Signal helpers and parameter state | `clip`, `sgn`, `ema`, `ema_bc`, `pstate`, `polyak` | the few primitives every engine and loop is written with, on top of `si`, `ba`, `ro`, `ma` |
| Losses and regularizers | `mse`, `pseudo_huber`, `logcosh`, `energy_loss`, `log_energy_loss`, `l2`, `l1s` | a loss is a plain Faust function `loss(y, t)`; these are smooth ones |
| Reparameterizations | `poles_from_reflection`, `reflection_from_poles`, `sigmoid_map` | learn in a domain where every value is valid (stable, positive, bounded) instead of clipping |
| Gradient conditioning and schedules | `clip_g`, `softclip_g`, `gate_g`, `lr_exp`, `lr_cos`, `warmup` | what happens to a gradient before the engine, and how a learning rate evolves |
| Least-squares engines | `lms`, `nlms`, `gn1`, `sgd`, `adam`, `rmsprop`, `nadam`, `sign_sgd` | engines that see the residual `r` and the sensitivity `j` separately |
| Gradient engines | `sgd_g`, `momentum_g`, `nesterov_g`, `adam_g`, `nadam_g`, `amsgrad_g`, `adabelief_g`, `rmsprop_g`, `adagrad_g`, `lion_g`, `sign_g` | engines that see one number, the loss gradient `g` |
| Least-squares loops | `lsq_1D` … `lsq_5D`, `optimize_1D` … `optimize_5D` | the model is differentiated, the loss is implicitly the squared error |
| Loss-first loops | `descend_1D` … `descend_5D` | the loss is differentiated, whatever it is |
| Gauss-Newton loops | `lm_2D`, `lm_3D` | second-order steps for two or three correlated parameters |
| Bus loops | `lsq_N`, `descend_N`, `descend_N_clocked` and `lsq_N_rad`, `descend_N_rad`, `descend_N_rad_clocked` | `N` parameters as a bus with one engine and one pair of bounds, in forward or in reverse mode |
| Clocked loops | `frame_sum`, `frame_count`, `frame_mean`, `descend_1D_clocked` … `descend_5D_clocked` | the gradient at audio rate, averaged over the frame, the step once per firing of an `ondemand` clock |
| Newton solver | `newton_step`, `newton` | not learning: solving an implicit equation with `F` and `F'` from one `fad` |

### 3.1 The shape of a loop

Every loop is the same recursion, drawn here for one parameter:

```text
              prev (recursive state)
                │
        pstate(init, reset, prev)     init on the first sample or on reset
                │
          clip(lo, hi, ·)            projection on the bounds
                │
                p ──────────────────────────────┐
                │                               │
      fad(loss(p), p) : !, _      or      fad(mdl(p, x), p) : (r, j)
                │                               │
            engine(g)                       engine(r, j)
                │                               │
          clip(lo, hi, p - step)  ──────────────┘
                │
              next  ──►  stored for the next sample
```

The parameter lives in Faust recursive state; `pstate` gives it an explicit
initial value and a reset control; `clip` keeps it in bounds; one `fad` call
per sample yields the derivative; the engine turns the derivative into a
step. With `N` parameters, one `fad` call with `N` seeds produces the `N`
derivatives at once. The bus loops draw the same picture with `N` wires in
place of one, and `rad` in place of `fad` in their `_rad` versions.

### 3.2 Two families, and why

The **least-squares** family (`lsq_ND`, `lm_ND`) differentiates the *model*
and hands each engine two numbers: the residual `r = model - target` and the
sensitivity `j = d(model)/dp`. The loss is implicitly `r^2`, but keeping `r`
and `j` separate is what makes normalization possible — NLMS divides the step
by the power of `j`, Gauss-Newton solves the normal equations built from the
`j`s — and normalization is the single most useful trick in adaptive
filtering.

The **loss-first** family (`descend_ND`) differentiates a scalar loss that the
user writes as an ordinary Faust function closed over the data, and hands each
engine one number, `g = d(loss)/dp`. Anything differentiable is a valid loss:
a robust one, an energy comparison that ignores phase, a model with several
outputs reduced to a scalar, a penalty on the parameters added to the error.
The price is that the engine no longer sees `r` and `j` separately, so it
cannot normalize by the sensitivity; adaptive engines (Adam, Lion) fill that
role.

The **bus loops** (`lsq_N`, `descend_N`, `descend_N_clocked`) are the same
two families for `N` parameters carried as a bus, `N` a constant, with one
engine and one pair of bounds for all of them — the shape of an adaptive FIR
or of a bank of gains — where the fixed-arity loops give each parameter its
own. Each has a `_rad` twin: one reverse sweep per sample for the `N`
derivatives instead of `N` tangents. Section 4.7 says what that sweep
computes through a recursion.

The original `optimize_ND` entry points are kept as wrappers of `lsq_ND`
(zero initial value, no reset).

### 3.3 The engine contract

An engine is a function whose last one or two arguments are the derivative
information; everything before is its settings. Partial application produces
the one- or two-argument remainder a loop expects:

```faust
op.descend_1D(loss, op.adam_g(0.01, 0.9, 0.999, 1e-8), lo, hi, init, reset);
op.lsq_3D(fir, op.nlms(0.02, 1e-6, 0.99), op.nlms(0.02, 1e-6, 0.99), op.nlms(0.02, 1e-6, 0.99), ...);
```

Each application is a separate instance with its own state, so two parameters
sharing the same engine expression do not share moments. Every learning rate
is a signal, which is why a schedule such as `op.lr_exp(...)` is simply passed
where a constant would be.

### 3.4 Standard libraries

The library imports `signals.lib`, `basics.lib`, `routes.lib` and
`maths.lib` (`si.smooth`, `si.bus`, `ba.time`, `ro.interleave`, `ma.PI`), so
the directory of the Faust standard libraries must be on the import path
next to `libraries`. The `faust-rs` test suite finds it through
`FAUST_RS_FAUSTLIBRARIES_ROOT` or a default checkout path and skips the
library's tests when neither exists, so the suite stays runnable without a
Faust distribution. Eleven fixtures in `tests/corpus/opt_*.dsp` run through
the interpreter in CI, one of them generated from the `#### Test` entry of
every documented function, so the documentation examples are compiled too.

## 4. Where the algorithms come from, and why these

### 4.1 Update engines

| Engine | Origin | Why it is here |
|---|---|---|
| `lms` | Widrow & Hoff, 1960 — the LMS adaptive filter | the ancestor of everything else; one multiplication |
| `nlms` | Nagumo & Noda, 1967 — normalized LMS | audio levels vary by 40 dB; dividing the step by the sensitivity power makes convergence independent of level. Measured on a 3-tap FIR: LMS is 100x too slow at level 0.1 and clamps at level 10, NLMS converges identically at 0.1, 1 and 10 |
| `gn1` | Gauss-Newton / recursive least squares | the one-parameter version of `lm_2D` |
| `sgd_g`, `momentum_g`, `nesterov_g` | Robbins & Monro 1951; Polyak 1964; Nesterov 1983 | the standard machine-learning steps; momentum averages noisy gradients |
| `adagrad_g` | Duchi et al., 2011 | a decaying step (the sum of squares only grows); useful when a parameter must settle for good |
| `rmsprop_g` | Tieleman & Hinton, 2012 | normalization by the recent gradient magnitude |
| `adam_g`, `nadam_g` | Kingma & Ba, 2015; Dozat, 2016 | the deep-learning default: momentum plus per-parameter normalization. The bias correction is implemented as `ema(a, g) / ema(a, 1)`, since `1 : smooth(a)` is exactly `1 - a^(n+1)`; without it the first steps are `3.16 * lr` |
| `amsgrad_g`, `adabelief_g` | Reddi et al., 2018; Zhuang et al., 2020 | Adam variants that never increase the effective step (AMSGrad) or normalize by the gradient's variance rather than its magnitude (AdaBelief) |
| `lion_g` | Chen et al., 2023 | steps of `±lr` in the direction of a momentum sign: one learning rate for parameters of any unit, one state variable. Measured: five biquad coefficients learned with a single Lion rate, all within 3e-6 of the target |
| `sign_g`, `sign_sgd` | sign descent | the simplest scale-free step |

The library keeps the original `sgd`, `adam`, `rmsprop`, `nadam`, `sign_sgd`
with their signatures; `adam` and `nadam` gained the bias correction.

Why so many? Because the parameters of an audio model have wildly different
units — a frequency in hertz, a quality factor, a filter coefficient in
`(-1, 1)` — and plain gradient descent needs one learning rate per unit.
Adaptive engines normalize each parameter's step by its own gradient history,
which is what lets a single `lr` serve a whole model. They are the pragmatic
answer; the principled one is the next family.

### 4.2 Second order: `lm_2D`, `lm_3D`

Gauss-Newton (Gauss, 1809, for orbits) uses the sensitivities `j` to build
the *normal equations* and solves them, which scales each parameter by its
own curvature and accounts for correlations between parameters. Levenberg
(1944) and Marquardt (1963) added damping so the step stays sane far from the
solution; Ljung & Söderström (1983) gave the recursive, forgetting-factor form
used in system identification (recursive prediction-error methods), of which
recursive least squares is the linear case.

`lm_2D` implements that recursive form: the information matrix is averaged
with a forgetting factor (bias corrected), damped by `lambda * diag(H)` so
the damping has no unit, and applied to the *instantaneous* innovation
`j * r`. Measured on `fi.resonlp(f, q)`: `(1200.000000, 2.000000)` from
`(1000, 1)` in 5 000 samples with a single gain, where the per-parameter
RMSProp of the synthesis note needed `lr_f = 2.0` and `lr_q = 0.01`. It is
limited to two and three parameters because Faust has no matrices; for the
interpretable models this library targets, that is usually enough. Note
that applying the step to the *averaged* innovation instead of the
instantaneous one would apply the same correction once per sample over the
whole window, and diverges.

### 4.3 Losses

| Loss | Origin | Why |
|---|---|---|
| `mse` | least squares | the default; its gradient is `2 r` |
| `pseudo_huber`, `logcosh` | Huber, 1964; log-cosh regression | quadratic near zero, linear far away, *smooth* everywhere: an outlier in the target moves the parameter by at most `lr`. Measured with `±20` spikes every 97 samples on a signal of amplitude 0.7: `mse` keeps a jitter of 0.2 on the gain, `logcosh` 0.007, `pseudo_huber` 0.0003 |
| `energy_loss`, `log_energy_loss` | power matching; the per-sample cousin of the spectral losses of DDSP | comparing smoothed powers ignores phase, so a model can be fitted to a target driven by a *different* realization of the excitation, where sample-wise error is meaningless. The log version is scale-free |
| `l2`, `l1s` | Tikhonov / ridge; lasso, smoothed | regularization as a loss term: `loss(p) = mse(...) + l2(0.001, p)` |

Smoothness is the selection criterion: `abs` and `max(0, ·)` appear nowhere
in a loss, because the derivative of `abs` is undefined at zero and the
compiler does not regularize it.

### 4.4 Reparameterizations

A learned coefficient of a recursive filter must stay in the region where
the filter is stable. Bounding `a1` and `a2` of a biquad to a rectangle does
not achieve that: the stability region is a triangle (`|a2| < 1`,
`|a1| < 1 + a2`), and the rectangle used by the original example admits
unstable points. Started from `(a1, a2) = (1.9, -0.5)` — inside the rectangle,
outside the triangle — the model diverges to `inf` and the projection can do
nothing about it. `poles_from_reflection` learns two *reflection
coefficients* `k1, k2` in `(-1, 1)` instead and maps them with
`a1 = k1 (1 + k2)`, `a2 = k2`: this is the lattice parameterization of
Itakura & Saito and Markel & Gray (1976), a bijection onto the triangle, so a
rectangular box on `(k1, k2)` contains only stable filters. `sigmoid_map`
replaces hard bounds by a logistic map, the way DDSP papers constrain their
parameters. The log-frequency recipe (`mdl(exp(u), x)` with bounds
`log(lo), log(hi)`) is the audio counterpart: a step in `u` is a relative
change in frequency, which is what the ear and the gradient both want.

### 4.5 Schedules, gating, readout

Learning-rate schedules (exponential decay; cosine annealing, Loshchilov &
Hutter 2017; warm-up) reconcile a fast start with a quiet end — in audio, the
"quiet end" is the absence of audible jitter on a parameter. `gate_g` learns
only when a condition holds, typically when there is signal: the same gating
adaptive filters use to avoid drifting in silence. `polyak` (Polyak & Juditsky,
1992) averages the parameter for the audible readout while the optimizer
keeps stepping on the raw one.

### 4.6 Newton

`newton` is not an optimizer: it solves `F(y) = 0` by Newton-Raphson, with
`F` and `F'` produced by one `fad` call. It is here because it is the same
primitive put to a different use, and because implicit equations are
everywhere in virtual-analog modelling (zero-delay-feedback filters, diode
clippers: Zavalishin, *The Art of VA Filter Design*).

### 4.7 Reverse mode inside a loop: pseudo-linear regression

A gradient consumed at the sample that produces it cannot wait for the end of
the block, so the reverse sweep of the `_rad` loops sees one sample: the
adjoint flows back through the model's operations of that sample and stops
at its recursive state, which is held fixed. For a recursive model
`y[n] = x[n] + p y[n-1]` that gives `d(loss)/dp = 2 r y[n-1]`, the *direct
term*; `fad` gives `2 r dy[n]/dp` with `dy[n]/dp = y[n-1] + p dy[n-1]/dp`,
the derivative through the recursion. In adaptive filtering the direct term
is the **pseudo-linear regression** gradient (Feintuch's IIR LMS, 1976;
Shynk 1989) and the recursive one the *recursive prediction error* gradient
(Ljung & Söderström 1983): the first is cheaper and converges to the same
solution when a positivity condition on the model holds, the second is the
exact descent direction. The library offers both — `fad` in the fixed-arity
loops and in `lsq_N`/`descend_N`, the direct term in the `_rad` twins — and
for a feed-forward model there is no difference at all, which is where
reverse mode earns its keep: many parameters, one sweep.

### 4.8 What was left out, and why

- **RLS / Gauss-Newton beyond three parameters**: Faust has no matrices;
  `lm_2D`/`lm_3D` cover the interpretable models this library targets, and
  larger parameter sets are better served by adaptive first-order engines.
- **AdamW** (decoupled weight decay): a regularizer is a loss term here,
  `l2(lambda, p)`, which composes with every engine.
- **L-BFGS and other batch methods**: they need a batch and a line search;
  online, one sample at a time, they have no natural form.
- **Spectral losses** (multi-scale STFT): they need a frame, which in Faust
  means an `ondemand` block; the fixture
  `tests/corpus/ondemand_fad_spectral_loss_008.dsp` shows `fad` through an
  FFT-based loss. Bringing it into the library is future work.

## 5. Measured behaviour

All runs: `faustprobe --double -I libraries -I <faustlibraries>`; programs
in the tutorial.

| Experiment | Result |
|---|---|
| Hand-written gradient vs `fad`, gain learned in a recursion | identical (difference `0`) |
| `descend_1D` + `adam_g(0.002)`, gain 0 → 0.7 | 0.6998 at 1 500 samples, exact at 2 500 |
| `lsq_1D` + `nlms`, one-pole `p* = 0.6` | 0.600013 at 500, exact at 2 000 |
| `lsq_3D` + `nlms` vs `lms`, 3-tap FIR, input level 0.1 / 1 / 10 | NLMS exact at all three; LMS 100x too slow at 0.1, clamped at 10 |
| Lion, single `lr = 0.001`, on `(f, q)` in hertz and Q | `q` converges, `f` crawls 1 Hz per 1 000 samples: the scale problem |
| Lion on `(log f, q)` | `(1206, 2.003)` at 10 000 samples, then ~2 % jitter |
| `lm_2D` on `(f, q)` | `(1200.000000, 2.000000)` at 5 000 samples |
| Five-coefficient biquad, rectangular `(a1, a2)` bounds, started at `(1.9, -0.5)` | diverges (`inf`) |
| Same with reflection coefficients, `descend_5D` + Lion | all five within 1e-5 of the target at 300 000 samples |
| `logcosh` vs `mse` under `±20` spikes | gain within 0.69–0.71 vs 0.49–0.88 |
| `log_energy_loss`, independent noise realizations, `fc* = 800` | 770–850 Hz (`mse` stays at the 20 Hz bound) |
| `newton(5)` on `y = tanh(x - 2y)` | residual `0` on every frame |
| `descend_1D` inside `ondemand`, one step every 64 samples | gain 0 → 0.7000 in 312 steps (20 000 samples) |
| audio-rate `fad` + `ema`, update in a 64-sample block | gain `0.700000` at 4 000 samples |
| `descend_1D` on an 8-point FFT loss inside the frame block | gain 0.34 (least-squares optimum 0.340) |
| `descend_1D_clocked`, SGD 0.5 per 64-sample frame | gain `0.700000` at 4 000 samples |
| `descend_2D_clocked`, Adam per frame on `(log f, q)` | `(1200.2, 1.996)` at 10 000 samples, then within 1 % |
| `lsq_N_rad` + `nlms`, 8-tap FIR at level 10 | residual below 1e-6 from 1 000 samples on |
| `descend_N` vs `descend_N_rad`, 16-tap FIR, LMS 0.02 | same residual to rounding; 3 777 vs 1 182 interpreter instructions, 0.10 s vs 0.04 s for 200 000 samples; 28 891 vs 4 129 and 1.32 s vs 0.13 s at 64 taps |
| in-graph `rad` vs `fad` on `y = 1 + p y[n-1]`, `loss = (y - 3)^2` | `rad` -3, -3.75, -3.94 (direct term), `fad` -3, -5, -6.19 (through the recursion) |

## 6. Pitfalls worth knowing

- **The seed must be the node used in the model.** Seeds are recognized by
  identity after lowering, not by algebraic equivalence. Inside a loop,
  `fad(loss(p), p)` with `p` the recursive input is the exact partial
  derivative.
- **A seed is matched by identity, and every occurrence counts.**
  `fad(F(v, v), v)` differentiates both occurrences of `v`: the Newton
  iteration of an implicit solver must not start from the very signal the
  equation holds fixed (`vprev`) — start from a predictor, or any distinct
  signal. Conversely a signal the seed does not reach (another loop's
  output, a noise generator) keeps a zero tangent and is not rewritten.
- **Sign convention.** With `r = model - target`, the MSE gradient is
  `+2 r j`. The synthesis note writes `err = target - model` and `-err * j`.
  Both are right; mixing them ascends the loss.
- **Smoothed losses have a delay.** `energy_loss` looks at a power averaged
  over `1 / (1 - a)` samples; the optimizer's own time constant must be
  longer, or the delayed feedback oscillates.
- **Adam and noisy losses.** Adam normalizes the step, so on a noisy loss the
  parameter random-walks by `lr` per sample; plain SGD, whose step follows
  the gradient magnitude, settles by itself. Schedules and `polyak` are the
  cure when Adam is still wanted.
- **Faust has no destructuring.** `(a1, a2) = poles_from_reflection(k1, k2)`
  does not parse; write `a1 = ... : _, !;`. And applying a five-output
  expression to a five-argument function is a partial application, not a
  spread: project each output.
- **Clock domains.** Inside an `ondemand` block, a recursion advances once
  per firing and `ma.SR` is not adapted; a body must receive outer signals as
  explicit inputs, and a frame operator with free `_` inputs must be given
  named arguments, or its inputs are duplicated at every use.
- **Double precision.** Gradients of recursive filters lose accuracy fast in
  single precision; compile learning programs with `-double`.
- **`op.mse(_, target)` has two inputs.** A free `_` is duplicated wherever
  the argument is used, so `(_ - t) * (_ - t)` is a two-input block and `:>`
  into it splits a bus between them: the taps random-walk near zero. Name
  the input: `\(y).(op.mse(y, target))`.
- **`rad` inside a loop sees one sample.** Through a recursion it returns
  the direct term, not the derivative through the recursion (section 4.7);
  learn recursive models with the `fad` loops, feed-forward ones with either.

## 7. References

- J. Engel, L. Hantrakul, C. Gu, A. Roberts, "DDSP: Differentiable Digital
  Signal Processing", ICLR 2020. <https://arxiv.org/abs/2001.04643>
- B. Hayes et al., "A Review of Differentiable Digital Signal Processing for
  Music and Speech Synthesis", Frontiers in Signal Processing, 2024.
  <https://arxiv.org/abs/2308.15422>
- A. G. Baydin, B. Pearlmutter, A. Radul, J. Siskind, "Automatic
  Differentiation in Machine Learning: a Survey", JMLR 2018.
  <https://arxiv.org/abs/1502.05767>
- R. J. Williams, D. Zipser, "A Learning Algorithm for Continually Running
  Fully Recurrent Neural Networks", Neural Computation, 1989.
- S. Haykin, *Adaptive Filter Theory*, Prentice Hall — LMS, NLMS, RLS.
- L. Ljung, T. Söderström, *Theory and Practice of Recursive
  Identification*, MIT Press, 1983 — recursive prediction-error methods.
- J. J. Shynk, "Adaptive IIR Filtering", IEEE ASSP Magazine, 1989 —
  pseudo-linear regression against recursive prediction error.
- P. L. Feintuch, "An Adaptive Recursive LMS Filter", Proc. IEEE, 1976.
- D. Marquardt, "An Algorithm for Least-Squares Estimation of Nonlinear
  Parameters", SIAM J. Appl. Math., 1963. <https://doi.org/10.1137/0111030>
- D. P. Kingma, J. Ba, "Adam: A Method for Stochastic Optimization", ICLR
  2015. <https://arxiv.org/abs/1412.6980>
- X. Chen et al., "Symbolic Discovery of Optimization Algorithms" (Lion),
  2023. <https://arxiv.org/abs/2302.06675>
- J. Zhuang et al., "AdaBelief Optimizer", NeurIPS 2020.
  <https://arxiv.org/abs/2010.07468>
- P. J. Huber, "Robust Estimation of a Location Parameter", Ann. Math.
  Statist., 1964. <https://doi.org/10.1214/aoms/1177703732>
- J. D. Markel, A. H. Gray, *Linear Prediction of Speech*, Springer, 1976 —
  reflection coefficients and the lattice form.
  <https://ccrma.stanford.edu/~jos/filters/Lattice_Ladder_Filters.html>
- V. Zavalishin, *The Art of VA Filter Design* — zero-delay feedback and
  implicit solvers.
- I. Loshchilov, F. Hutter, "SGDR: Stochastic Gradient Descent with Warm
  Restarts", ICLR 2017. <https://arxiv.org/abs/1608.03983>
- Faust-side notes: [docs/fad-note-en.md](../docs/fad-note-en.md),
  [docs/ondemand-note-en.md](../docs/ondemand-note-en.md),
  [docs/rad-note-en.md](../docs/rad-note-en.md),
  [docs/rad-usage-en.md](../docs/rad-usage-en.md),
  [docs/fad-rad-synthesis-en.md](../docs/fad-rad-synthesis-en.md),
  [docs/fad-debruijn-recursion-en.md](../docs/fad-debruijn-recursion-en.md).
