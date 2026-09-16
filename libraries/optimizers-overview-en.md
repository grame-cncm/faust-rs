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

The file [optimizers.lib](optimizers.lib) (prefix `op`, version 0.9.0) is
documented function by function in the Faust libraries convention; this
section gives the map. It has fifteen sections, ordered from building blocks to
ready-made loops and what surrounds them.

| Section | What it holds | Why it exists |
|---|---|---|
| Signal helpers and parameter state | `clip`, `sgn`, `ema`, `ema_bc`, `pstate`, `polyak`, `init_latch`, `init_reset`, `stalled`, `no_progress` | the few primitives every engine and loop is written with, on top of `si`, `ba`, `ro`, `ma`; starting a loop from an outside estimate; detecting a flat plateau (`stalled`) or a sloped one (`no_progress`) |
| Losses and regularizers | `mse`, `pseudo_huber`, `logcosh`, `energy_loss`, `log_energy_loss`, `l2`, `l1s` | a loss is a plain Faust function `loss(y, t)`; these are smooth ones |
| Reparameterizations | `poles_from_reflection`, `reflection_from_poles`, `sigmoid_map` | learn in a domain where every value is valid (stable, positive, bounded) instead of clipping |
| Gradient conditioning and schedules | `clip_g`, `softclip_g`, `gate_g`, `ramp_lin`, `ramp_exp`, `lr_exp`, `lr_cos`, `warmup` | what happens to a gradient before the engine, and how a learning rate, or a model parameter, evolves |
| Least-squares engines | `lms`, `nlms`, `gn1`, `sgd`, `adam`, `rmsprop`, `nadam`, `sign_sgd` | engines that see the residual `r` and the sensitivity `j` separately |
| Gradient engines | `sgd_g`, `momentum_g`, `nesterov_g`, `adam_g`, `nadam_g`, `amsgrad_g`, `adabelief_g`, `rmsprop_g`, `adagrad_g`, `lion_g`, `sign_g`, `langevin_g` | engines that see one number, the loss gradient `g`; `langevin_g` adds an annealed noise to it, to leave a shallow well |
| Least-squares loops | `lsq_1D` … `lsq_5D`, `optimize_1D` … `optimize_5D`, `lsq_1D_restart` | the model is differentiated, the loss is implicitly the squared error; `lsq_1D_restart` changes start when the residual stops making progress |
| Loss-first loops | `descend_1D` … `descend_5D`, `descend_1D_restart` | the loss is differentiated, whatever it is; `descend_1D_restart` takes the next start when the loss stops making progress |
| Gauss-Newton loops | `lm_2D`, `lm_3D` | second-order steps for two or three correlated parameters |
| Bus loops | `lsq_N`, `descend_N`, `descend_N_clocked` and `lsq_N_rad`, `descend_N_rad`, `descend_N_rad_clocked` | `N` parameters as a bus with one engine and one pair of bounds, in forward or in reverse mode |
| Clocked loops | `frame_sum`, `frame_count`, `frame_mean`, `descend_1D_clocked` … `descend_5D_clocked` | the gradient at audio rate, averaged over the frame, the step once per firing of an `ondemand` clock |
| Gradient-free loops | `spsa_1D_clocked`, `spsa_N_clocked`, `search_1D_clocked` | learning with no tangent at all, from two evaluations of the loss per frame: an integer delay, a `select2`, anything `fad` differentiates to zero |
| Multi-start loops | `grid_init`, `multistart_1D`, `multistart_lsq_1D`, `grid_then_descend_1D` | several starts at once: `K` descents in parallel, following the best, or `K` candidates scored with no tangent, then one descent from the best |
| Gating and stopping | `gated`, `gated_when`, `stop_after`, `stop_below`, `stop_relative`, `on_change` | switching the learning off once it has converged, so that it costs nothing afterwards; computing coefficients only when a parameter changes |
| Newton solver | `newton_step`, `newton` | not learning: solving an implicit equation with `F` and `F'` from one `fad` |

### 3.1 The shape of a loop

Every loop is the same recursion, drawn here for one parameter:

```text
              prev (recursive state)
                │
        init + deviation             the recursion keeps the deviation from init; the step applies to it; reset clears it
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

The parameter lives in Faust recursive state as its deviation from the
initial value: the recursion starts at zero, `reset` clears it, and no
first-sample detection is involved, so the loop also works inside an
`ondemand` block whatever its first firing. The step is applied to the
deviation and clipped in deviation space, so a step smaller than the
precision of the value is not lost (a parameter near 1000 learned in single
precision keeps steps of 1e-5); one `fad` call
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
| `langevin_g` | Welling & Teh, 2011 — stochastic gradient Langevin dynamics | the SGD step plus a noise of standard deviation `sqrt(2 lr temp)`: at a fixed temperature the parameter samples `exp(-loss / temp)`, annealed to zero it explores then descends. Measured on a two-well loss, `(p² - 1)² + 0.3 p` from the shallow well: SGD stays there (0.960), Langevin crosses the barrier and cools into the deep well (-1.036); at zero temperature it is SGD bit for bit |

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
"quiet end" is the absence of audible jitter on a parameter. A schedule is a
signal: `ramp_lin` and `ramp_exp` are the same ramps under a neutral name
(`lr_exp` is `ramp_exp`, bit for bit), to anneal a *model* parameter — a
damping, the smoothing of a loss — and not only a rate; this is the
continuation of section 9, written as a signal. `init_latch` and
`init_reset` make an outside estimate the `init` of a loop: the loop is held
at `init` while the estimate is observed, then released on the frozen value;
`stalled` reads a flat plateau, a small gradient under a high loss, and
`no_progress` a sloped one, a loss that no longer falls over a patience
window: the latter is what `descend_1D_restart` restarts on, since on the
string the gradient is larger on the plateau than in the well (section 5). `gate_g` learns
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

### 4.9 Gating and stopping

A block that has converged keeps costing what it cost while learning: the
model carrying the tangents, the loss and the engine run at every sample.
The only way not to compute something in Faust is a clock domain, since
`select2` evaluates both branches and `gate_g` zeroes a gradient it has
already computed; `ondemand(C)` computes nothing while its clock is silent
and holds its outputs. `gated(C)` is that, applied to a block whose last
output is a flag: the clock is `1 - flag'`, the block runs on every sample
until the flag rises, then never again. The one-sample delay of the
recursion is what makes the construct legal (a clock cannot depend on the
block's output at the same sample) and what makes a flag raised on a
period's last sample stop the block on the period boundary. `gated_when`
adds an enable signal; both need `outputs(C)`, which is why they are
written with `route` rather than with a fixed arity.

The flag is the criterion's business, not the gate's, hence the separate
`stop_*` functions, all built on `frame_sum` over the period clock and on
`ba.peakhold(1)`, the running maximum of the standard library, which keeps
a flag raised: a period budget (`stop_after`), a loss threshold (`stop_below`),
and `stop_relative`, which compares the loss of a period with the loss at
the previous checkpoint `window` periods earlier and stops when their
relative change is under `tol`, after `min_periods`, or at `max_periods`.
Consecutive periods are not compared: on a loss with an overshoot, its
plateau looks like convergence for a few periods and a one-period test
fires there; the checkpoints are what make the test robust.

`on_change(C)` is the other half of the saving. Once stopped, the learned
parameters are still signals, and a filter computing its coefficients from
them recomputes `exp`, `tan` or `cos` at every sample, where the compiler
moves the same expressions out of the sample loop when they depend on
sliders. `on_change` runs `C` in an `ondemand` whose clock is the comparison
of every input with its previous value, plus the first sample: once per
optimiser step while learning, never afterwards. It needs filters that take
coefficients rather than parameters; `fi.filterbank(1, (fx)) : *(g), _ :> _`
is the shelf of `fi.highshelf(1, L, fx)` with a linear gain. Section 7 has
the measurements: the gate divides the cost of a self-calibrating
reverberator by about nine, `on_change` brings it to the cost of the
reverberator alone.

### 4.10 Gradient-free: `spsa_1D_clocked`, `spsa_N_clocked`, `search_1D_clocked`

Everything above differentiates. These three loops never do: the loss is
evaluated at two parameter values over each frame, on the same excitation,
and the update takes the difference. Spall's simultaneous perturbation (1992)
holds a sign `delta` over the frame, evaluates `L(p + c delta)` and
`L(p - c delta)`, and hands `(L+ - L-) / (2 c delta)` to the engine, the
ordinary contract; on a quadratic loss the estimate is the exact frame
gradient, and the loop follows `descend_1D_clocked` to rounding (section 5).
For `N` parameters, one vector of `N` signs and still two evaluations, where
coordinate-wise finite differences would need `2N`. Rechenberg's (1+1)
evolution strategy (1973) keeps a candidate `p + sigma u` next to the
incumbent and adopts it when its frame loss is strictly lower; no engine, the
step is the acceptance. What they reach is what `fad` cannot see: an integer
delay length (measured: a comb whose integer delay goes from 160 to 200
samples), a `select2` on the parameter (measured: the branch found within a
few frames while `descend_1D` never moves), a written table. The price: two
model copies instead of one copy and its tangent, one step per frame, and a
noisy estimate that wants a `c` or `sigma` on the parameter's scale.

### 4.11 Several starts: `multistart_1D`, `multistart_lsq_1D`, `grid_then_descend_1D`

When no estimate says which basin holds the answer, start from several
places. `multistart_1D` and its least-squares twin run `K` descents in
parallel, each with its own engine, and follow the one whose smoothed loss
is lowest, the first on ties; the price is `K` models and their tangents.
Measured on the string: four NLMS loops from 176, 200, 228 and 264 Hz, only
the one from 228 Hz locks, and the loop follows it from 16 000 samples on.
`grid_then_descend_1D` scores `K` fixed candidates for `T` samples with no
tangent at all, then runs one descent from the best, frozen by `init_latch`:
DDSP's detector-then-gradient scheme written by hand, for `K` models during
the window and one model plus its tangent afterwards (the candidates keep
running: a latched branch is not pruned). The grid sees a basin only when its
spacing is finer than the basin: on the string, whose well is ±1 Hz wide, a
grid over the whole range would need hundreds of cells, where four starts
spread over the range include one in the capture zone; it is measured on the
two-well loss, where eight cells suffice.

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
| `init_latch` + `init_reset` on the string, autocorrelation estimate observed for 8 192 samples | init frozen at 222.77 Hz (+1.3 %), pitch 219.998 at 24 000, `220.000000` from 48 000 on, residual under 1e-6 |
| `stalled(0.999, 0.01, 0.1)` on (gradient, loss) = (0.5, 1), (0, 1), (0, 0.001) | 0, 1, 0 per segment |
| `lr_exp` vs `ramp_exp` | bit-identical |
| `langevin_g`, temperature annealed 0.5 → 0, vs `sgd_g`, two-well loss from the shallow well | SGD 0.960 (shallow well), Langevin -1.036 (deep well) at 200 000 samples; at temperature 0, identical to SGD |
| `spsa_1D_clocked` vs `descend_1D_clocked`, gain, SGD 0.5 per 64-sample frame | same trajectories, difference `0` on every sample, 0.700000 at 4 000 |
| `spsa_1D_clocked`, integer delay of a comb, `c = 2`, Adam 0.5 per 256-sample frame, from 160 | `int(d) = 200` from 25 000 samples, held from 40 000 on, residual 0; the `fad` tangent is identically zero |
| `search_1D_clocked` vs `descend_1D`, `select2(p > 0.5, …)` from 0 | the search at 0.83 within a few frames, loss 0; `descend_1D` never moves |
| `descend_1D` + Adam 0.02 on the string, smoothed gradient and loss | from 228 Hz: locked on 220 by 18 000 samples, gradient 0.003, loss 1e-4; from 200 Hz: walks between 196 and 207 Hz, gradient 0.025, loss 0.011, larger on the plateau than in the well |
| `multistart_lsq_1D(4, (176, 200, 228, 264 Hz))` + NLMS on the string | index 2 (228 Hz) from 16 000 samples on, `220.000000` Hz; four strings and their tangents compile in 58 ms |
| `multistart_1D(4, grid_init(4, -3, 3))` + SGD on the two-well loss | the two left starts end in the deep well, the loop follows one of them, -1.036 |
| `grid_then_descend_1D(8, 2 000, grid_init(8, -3, 3))` on the two-well loss | the cell at -1.125 (index 2) chosen at 2 000 samples, the descent at -1.036 |
| `descend_1D_restart(2, (1, -1))` + SGD on the two-well loss, patience 4 000 | 0.960 (shallow well, loss 0.29 > `eps_l`) then a restart at 8 000 and -1.036 (deep well) for good |
| `lsq_1D_restart(2, (1, -1))` + NLMS on `x · L(p)` against `-0.2 x`, `L` the two-well polynomial | 0.960 (residual 0.24 E[x²], never nil in the shallow well) then a restart at 8 000 and a root of `L = -0.2` in the deep well, residual nil |
| a restart on the string from 228 Hz after a drift, NLMS or Adam, single or double precision | does not lock the way a fresh loop from 228 does: the model, its tangent and the engine keep the drift's state; not kept as a fixture |
| `no_progress(2 000, 0.05, 0.1)` on a decaying, a constant high and a constant low loss | 0, 1, 0 per segment |

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
- **An `init` computed in the graph once weighed on compile time.** An
  `init` of thirty autocorrelation lanes multiplied by a hundred the compile
  time of a loop that closed over it; that was three unmemoized walks of the
  compiler, fixed on 2026-09-16, and every loop now accepts such an `init`.
  The one-parameter loops (`lsq_1D`, `descend_1D`, `descend_1D_clocked`)
  take it through an input wire since 0.9.0, the cleaner form.
- **`rad` inside a loop sees one sample.** Through a recursion it returns
  the direct term, not the derivative through the recursion (section 4.7);
  learn recursive models with the `fad` loops, feed-forward ones with either.

## 7. Two phases: learning, then use

A program that learns inside the audio process keeps paying for its
learning after the parameters have settled: the network carrying the
tangents, the loss and the optimiser run at every sample whether or not
they are still useful, and with one augmented lane per parameter they are
most of the cost: with a dozen parameters, the effect itself is a few
percent of it. Gating the gradient with `gate_g` does not help: the gradient is still computed, then zeroed. A
`select2` does not help either, Faust evaluates both branches. What does is
`ondemand`: a block whose clock does not fire computes nothing and holds
its outputs.

**The gate.** `gated(C)` (section 4.9) runs an arbitrary block `C` whose
last output is a flag in an outer `ondemand` whose clock is `1 - flag'`;
while the flag is 0 the block runs on every sample, once it is 1 nothing of
it is computed and its outputs hold. Put the whole learning in `C`, the
loop included, and let a `stop_*` criterion raise the flag; the effect that
processes the audio reads the held parameters and nothing else changes:

```faust
learn(t) = ps <: (si.bus(P), (loss(t) : op.stop_relative(clock, 20, 40, 300, 0.02)))
with { ps = op.descend_N_clocked(P, clock, loss(t), op.adam_g(0.03, 0.9, 0.999, 1e-8), lo, hi, 0.0, 0.0); };
params = t : op.gated(learn);      // P held parameters, then the flag
```

`stop_relative` compares the loss of a period with the loss at the previous
checkpoint, `window` periods earlier, and stops at a period boundary when
the relative change is under `tol`, after `min_periods`, or at
`max_periods`; comparing consecutive periods instead is fooled by the
plateaus of an overshoot. `stop_after` and `stop_below` are the simpler
budgets, a number of periods or a loss threshold, and `gated_when` adds an
enable signal. The recursion's one-sample delay makes the clock fall at the
sample after the period's last one, so the block's own time, which counts
its firings, stays aligned with the period if learning is resumed. The
period losses of the gated program are bit-identical to the ungated one's:
a `fad` and a recursion inside an `ondemand` inside another `ondemand` are
compiled exactly.

**The coefficients.** Once stopped, the held parameters are still signals,
so a filter that computes its coefficients from them recomputes `exp`, `tan`
and `cos` at every sample, where the compiler moves the same expressions
out of the loop when they depend on sliders. `on_change(C)` closes the
gap: it runs `C`, the computation of the coefficients (shelf and equaliser
gains, cosines and sines of learned angles), in an `ondemand` whose clock
is the comparison of every input with its previous value, so it fires once
per optimiser step and never once learning has stopped; the filters take
the held linear gains (`fi.filterbank(1, (fx)) : *(g), _ :> _` is what
`fi.highshelf` is built on). The response is bit-identical to the
per-sample version.

**Measured** on one core, blocks of 320 samples, a reverberator with a
dozen learned parameters (the calibration example of the DDSP examples
document, taken further):

| phase | × real time |
|---|---|
| learning, ungated program | 28 |
| learning, gated program | 23 |
| after the gate | 196 |
| after the gate, coefficients hoisted | 572 |
| the same network with sliders, no learning | 625 |
| after a recompilation with the learned values as constants | 631 |

The gate costs about 18 % while learning, the nesting of the domains; once
stopped the effect costs no more than the same network alone. The last row
is the other route, the one a tensor framework has to take: the host
substitutes the learned values for the sliders, recompiles (0.07 s with the
Cranelift JIT) and swaps the instance, which then starts from silence and
has to be crossfaded; constant propagation gains nothing over sliders,
since slider-dependent expressions already leave the sample loop. The gate
needs no host and keeps the state; it is `ondemand` applied to the
derivative, available because the derivative is a signal of the same
program, and clocks decide which parts of it are computed: the learning
while it is useful, the coefficients when a parameter changes, the effect
always.

## 8. Scope: what this reaches, and what it does not

Against the tensor frameworks of DDSP (PyTorch or JAX with the DDSP
libraries, torchaudio, FLAMO, dasp-pytorch), compiler-level differentiation
is to DDSP what adaptive filtering is to machine learning: exact, cheap,
real time, interpretable, small. Its domain is the parametric models whose
parameters a sound engineer can read. The calibration of a reverberator to
measured rooms, offline and inside the audio process, is the worked example
behind this section, which is what it taught about the reach of the
approach.

**What can reasonably be reached.**

- *Calibration and system identification*: reverberators, filters,
  equalisers, physical models, grey-box circuits, with tens to a few hundred
  parameters. `rad` costs about three forward passes whatever their number,
  so a few hundred stay affordable offline.
- *Learning inside the audio process*, which no framework does: effects that
  calibrate themselves, tracking a drifting target, echo cancellation,
  adaptive filters, patches that tune themselves, and the learning switched
  off by an `ondemand` clock once done, at no cost afterwards (section 7). Realistic up
  to a few tens of parameters in real time with `fad`, on embedded targets
  or in a browser, since the program that learns is ordinary Faust.
- *Small networks written in Faust*: an amplifier GRU, an MLP of a few
  hundred weights (examples 5 and 9 of `ddsp-examples-en.md`). Trainable,
  but slowly: on a CPU, one example at a time.
- *Design by objective*: the parameters of a fixed structure that reach a
  specification where no analytical formula exists; and Newton solvers for
  implicit circuits, already in place.
- *The bridge with the frameworks*, within reach but not done: a Faust
  program as a differentiable layer in PyTorch. `rad` produces the
  vector-Jacobian products an `autograd.Function` expects, and in the other
  direction an encoder trained in PyTorch exports to Faust. Each side gets
  what it lacks.

**The current limits, the ones that can be worked on.**

- *No batches, no accelerator.* One instance processes one signal, sample
  by sample, on one core; on a dataset of hours the distance to a framework
  is orders of magnitude.
- *Everything is a signal graph.* A layer of a thousand weights is a
  thousand signals; compile time and code size grow with the differentiated
  graph (forty forward tangents through a small reverberator: 15 s). Beyond a few
  tens of thousands of nodes, compile-time differentiation stops following.
- *No FFT in the language.* The multi-resolution spectral loss, the
  workhorse of DDSP, does not exist as such; filter banks approximate it.
- *The bounds of `rad`.* Tapes proportional to the block, so memory is block
  length times recorded signals; a horizon equal to the block, with a zero
  adjoint at its end and nothing carried across blocks, hence exact over a
  whole response in one `compute` but truncated to the buffer in a stream,
  which is why streaming learners use `fad`; no variable delay (`fad` has
  it); no writable table nor soundfile (`rdtable` is differentiated with
  respect to its index only); no crossing of a clock-domain boundary; the
  derivative of the active branch at `select2`, `min`, `max`, zero for
  integer and bitwise operations; no second derivative (neither `fad` over
  `rad` nor `rad` over `fad`), so no Hessian-vector products, though the
  Jacobian columns from `fad` allow a Gauss-Newton step when the parameters
  are few.
- *Tooling.* No execution graph to inspect at run time, no `.grad` on a
  node; `faustprobe` renders any lane and the signal DAG can be dumped, but
  finding the source of a NaN means bisecting the source. No learning-rate
  schedule is provided (a schedule is a signal and can be written), no
  checkpoint (an instance's state cannot be saved and restored), no
  hyperparameter search beyond a host loop over compilations. Double
  precision stays necessary: tangents and adjoints through thousands of
  samples of recursion lose digits fast in single precision, while plugins
  usually run in single.

**What this approach will not do, by construction.**

- *Large-scale deep learning*: millions of parameters, corpora of hours,
  neural codecs, diffusion models, the big convolutional amplifier models.
  The one-signal-per-node representation, one-instance execution and the
  absence of tensors and GPU exclude them; inference of medium networks in
  Faust stays possible, not their training.
- *Dynamic graphs*: Faust is static dataflow, no data-dependent shape, no
  variable length other than through clocks, no recursion over structures;
  hence no transformers, beam search or tree-structured models.
- *Learning representations from a corpus*: the strength of Engel et al.'s
  DDSP is a neural encoder learned on data coupled with the differentiable
  synthesiser; Faust can carry the second half, never the first.
- *Differentiating what is not a signal*: the topology, the number of
  lines, an integer delay length, a discrete choice. As in every framework
  this needs relaxations, and they would be written in Faust.

## 9. Non-convexity: what gradient descent asks of the landscape

Gradient descent guarantees the global minimum only for a *convex* loss: a
single basin, into which every starting point descends. Almost no loss in
this document is convex, and yet almost every program converges. Convexity
is therefore not what separates what learns from what does not. Three
questions do: is the starting point inside the basin of the right minimum;
is the gradient informative there, or is the landscape flat; is the problem
conditioned, that is, do the parameters have comparable scales. This
section rereads the examples of the repository through those three
questions, then says what the library offers when the landscape is hard,
and what it does not.

**What is convex.** A model *linear in its parameters* under a squared error
gives a quadratic loss, a bowl: a gain, a bias, the coefficients of an FIR,
the amplitudes of a harmonic bank. That is the domain of LMS and its
relatives, the 64-tap echo canceller, the bus loops, the harmonic
synthesizer. These programs converge from any starting point; only the
speed depends on conditioning, which the normalisation of `nlms` settles.
Even there one trap remains: a loss on *magnitudes* is blind to sign and has
two symmetric minima, `a` and `−a`; example 11 of `ddsp-examples-en.md`
removes one with `a_h = exp(p_h)`, a reparameterisation that leaves a single
minimum.

**What is not.** As soon as a parameter enters a recursion or a frequency,
the loss stops being convex: the frequency and Q of a resonator, the poles
of a biquad, the `c = cos w` of a notch, the T60 of an FDN, the delay length
of a string, the weights of a GRU or an MLP. The output error of a recursive
filter can have local minima (Stearns 1981; Söderström & Stoica 1982),
especially when the model is of insufficient order. Yet the notch converges
from 1400 Hz to 1000, `resonlp` from `(1000, 1)` to `(1200, 2)`, the FDN
from `(0.3 s, 0)` to `(0.6, 0.3)`, the biquad from zero to its target.
Convexity is not what saves them: a wide enough basin and a starting point
inside it are.

**The measured counter-example.** The waveguide string (example 10 of
`ddsp-examples-en.md`) shows the landscape itself. The waveform error
between two strings is a well ±1 Hz wide around 220 Hz on a flat plateau.
From 228 Hz the pitch locks to 220.000000; from 200 Hz it drifts to 190.
Same model, same loss, same optimizer: only the starting point changes.
This is why Engel et al.'s DDSP has f0 estimated by a detector and lets the
gradient refine it; DDSP is not a method for convex problems, it is a set
of techniques that make a non-convex landscape passable by gradient.

**What the library offers for a hard landscape.** Each tool acts on one of
the three questions.

- *The starting point.* The `init` of every loop is the first tool, and the
  strongest: an outside estimate, a pitch detector, the value from a
  previous session. `init_latch(T, e)` and `init_reset(T)` make an estimate
  observed for `T` samples that `init`: on the string, the target's
  autocorrelation peak, frozen 2 % above, brings the pitch to `220.000000`
  with no start chosen by hand (section 5). `on_change` and the `reset`
  input allow a restart when the target jumps.
- *Reparameterisation.* It changes the shape of the landscape without
  moving its minimum. Frequency in log makes the steps relative; reflection
  coefficients turn the stability triangle into a box, so every point
  reached is a valid filter; `exp` on an amplitude removes the mirror
  minimum; `sigmoid_map` replaces a hard bound, where the gradient is lost,
  by a slope.
- *The loss.* A waveform error compares phases, hence narrow wells;
  `energy_loss` and `log_energy_loss` compare smoothed powers and widen the
  basin. Section 7.2 of the tutorial measures it: on two independent
  excitations, `mse` leaves the cutoff stuck at the 20 Hz bound,
  `log_energy_loss` brings it back between 770 and 850 Hz around the 800 Hz
  target. A per-frame spectral loss, inside an `ondemand` block, is the next
  step; the multi-resolution version, DDSP's standard tool for widening
  basins in frequency, is the pending work of section 4.8. Robust losses
  (`logcosh`, `pseudo_huber`) do not change the shape of the basin, they
  bound the blows outliers deal to it.
- *Continuation.* Start on a smooth landscape and harden it along the way:
  the string's damping annealed from 0.70 to 0.95 (broad resonances first)
  makes convergence from 264 Hz possible where only 228 Hz worked; the
  notch radius `r` likewise sets the width of the basin (0.9 wide, 0.99
  narrow). `ramp_lin` and `ramp_exp` write that annealing as a signal; a
  learning-rate schedule, `lr_exp` or `lr_cos`, is the simplest version of
  it: explore fast, then settle.
- *Second order.* `lm_2D` and `lm_3D` settle conditioning, not
  multimodality: a Gauss-Newton step descends into the basin it is in, only
  faster and without a rate per parameter. Marquardt's damping is what keeps
  it reasonable where the quadratic approximation is wrong, far from the
  solution.
- *The direct term.* The `_rad` loops (section 4.7) do not change the
  landscape either: their convergence rests on a positivity condition, not
  on the shape of the loss.

**When the gradient is not enough.** A landscape with several basins and no
good initialisation calls for a search that the gradient does not do, and
that usually wraps it rather than replaces it:

- *several starts*: the same descent launched from distinct points, keeping
  the one whose smoothed loss is lowest;
- *a coarse grid, then the gradient*: sweep the hard parameter, the pitch or
  a delay length, in wide steps, and refine by descent from the best point;
  this is DDSP's detector-then-gradient scheme, written by hand;
- *gradient-free methods*: simulated annealing, CMA-ES (Hansen 2016),
  Nelder-Mead, Bayesian optimisation; they evaluate only the loss and suit
  few parameters, offline, where the gradient is zero or misleading;
- *discrete parameters*: an integer delay length, a topology, a choice;
  they have no derivative (section 8) and must be relaxed or enumerated.

Of all this the library has the building blocks — the `init` from an
estimate, the ramps, the plateau detector `stalled` (a small gradient under
a high loss), `langevin_g`, the SGD step plus an annealed noise, which
leaves a shallow well (section 5) but has no pull on a plateau — and the
gradient-free loops of section 4.10, `spsa_1D_clocked`, `spsa_N_clocked` and
`search_1D_clocked`, which learn discrete parameters from two evaluations of
the loss per frame; and `descend_1D_restart`, the sequential multi-start for
the cost of one model, which takes the next start when the loss stops making
progress (measured on the two-well landscapes: the shallow well left at
8 000 samples for the deep one; on the string, a start taken after a drift
does not lock the way a fresh loop does, section 5); and the multi-start
loops of section 4.11, `multistart_1D`, `multistart_lsq_1D` and
`grid_then_descend_1D`, the two forms this paragraph used to announce. In the graph,
`multistart_1D` is the first one as such, `K` loops in parallel, a loss
smoothed by `ema_bc` for each and a selector that follows the best;
switching the losers off with `gated` remains a composition to write by
hand. On the host side, the loop of
[docs/rad-usage-en.md](../docs/rad-usage-en.md) recompiles and writes
parameters through `set_real_zone`, and a multi-start or a Bayesian search
over inits still grafts onto it without touching the compiler.

**Convex does not mean easy.** Online, one sample at a time, a quadratic
bowl is crossed as badly as any other landscape when the step is wrong:
Adam's random walk on a noisy loss, the oscillation of a smoothed loss
slower than the optimizer, a parameter in hertz and another without unit
under a single rate. Those walls are the ones of section 6 and of section
13 of the tutorial, and they have nothing to do with convexity.

## 10. References

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
- S. D. Stearns, "Error Surfaces of Recursive Adaptive Filters", IEEE
  Trans. ASSP, 1981 — local minima of the output error of a recursive
  filter.
- T. Söderström, P. Stoica, "Some Properties of the Output Error Method",
  Automatica, 1982 — unimodality and local minima of the output error.
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
- M. Welling, Y. W. Teh, "Bayesian Learning via Stochastic Gradient Langevin
  Dynamics", ICML 2011 — the `langevin_g` engine.
- J. C. Spall, "Multivariate Stochastic Approximation Using a Simultaneous
  Perturbation Gradient Approximation", IEEE Trans. Automatic Control, 1992
  — `spsa_1D_clocked`, `spsa_N_clocked`.
- H.-G. Beyer, H.-P. Schwefel, "Evolution Strategies: A Comprehensive
  Introduction", Natural Computing, 2002 — `search_1D_clocked`.
- N. Hansen, "The CMA Evolution Strategy: A Tutorial", 2016 — gradient-free
  search. <https://arxiv.org/abs/1604.00772>
- Faust-side notes: [docs/fad-note-en.md](../docs/fad-note-en.md),
  [docs/ondemand-note-en.md](../docs/ondemand-note-en.md),
  [docs/rad-note-en.md](../docs/rad-note-en.md),
  [docs/rad-usage-en.md](../docs/rad-usage-en.md),
  [docs/fad-rad-synthesis-en.md](../docs/fad-rad-synthesis-en.md),
  [docs/fad-debruijn-recursion-en.md](../docs/fad-debruijn-recursion-en.md).
