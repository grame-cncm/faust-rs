# Learning DSP parameters inside Faust: a beginner's tutorial

French version: [optimizers-ddsp-tutorial-fr.md](optimizers-ddsp-tutorial-fr.md)
(same content; keep both versions synchronized). Background and design
rationale: [optimizers-overview-en.md](optimizers-overview-en.md).

This tutorial assumes you can read and write ordinary Faust and nothing else.
By the end you will have written programs that learn a gain, a filter pole, a
resonant filter's frequency and Q, and the five coefficients of a biquad, all
inside the Faust graph, and you will know which tool to reach for when
something does not converge. Every program was run on the current compiler;
the numbers you should see are given after each one.

## 0. Setting up

Compile with the project-local library directory on the import path, and in
double precision — gradients of recursive filters lose accuracy fast in
single precision:

```sh
faust-rs -double -I libraries -lang cpp program.dsp
```

To *see* a program learn without wiring audio, `faustprobe` renders it offline
and prints selected frames and statistics. All examples below were checked
with it; replace `<faustlibraries>` by the directory holding `stdfaust.lib`:

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 3000 --every 500 program.dsp
```

`-n` is the number of frames rendered, `--every` prints one frame out of N,
`--quiet` prints only the per-output statistics, `--in sine:220` feeds a sine
where the program has an input. Read
[docs/faustprobe-user-guide-en.md](../docs/faustprobe-user-guide-en.md) for the
rest.

The library is loaded with a prefix:

```faust
op = library("optimizers.lib");
```

## 1. The smallest learning loop, by hand

Start with a gain. Some hidden system multiplies a signal by `0.7`; we hear its
output (the **target**) and the input, and we want our own gain `g` to match.

Four ideas, in the order they appear in the code:

- **model**: what we compute, `g * x`;
- **loss**: how wrong we are at this sample, `(g * x - target)^2` — squared so
  that it is positive and smooth;
- **gradient**: how the loss changes when `g` changes. For this loss it is
  `2 (g x - target) x`: positive when `g` is too large, negative when too
  small;
- **update**: move `g` against the gradient, `g <- g - lr * gradient`, where
  the learning rate `lr` sets the step size.

The update needs memory: the new `g` depends on the previous one. In Faust,
memory is recursion, and `~ _` feeds the previous output back as the input of
the next sample. Here is the loop written by hand, and next to it the same
loop where `fad` computes the derivative instead of us:

```faust
import("stdfaust.lib");
x = no.noise;
target = 0.7 * x;
lr = 0.01;
loss(g) = (g * x - target) * (g * x - target);
// gradient written by hand: d/dg (g x - t)^2 = 2 (g x - t) x
g_manual = (\(g).(g - lr * 2.0 * (g * x - target) * x)) ~ _;
// the same gradient computed by fad
g_fad = (\(g).(g - lr * (fad(loss(g), g) : !, _))) ~ _;
process = g_manual, g_fad, g_manual - g_fad;
```

`fad(loss(g), g)` returns two signals, the loss and its derivative with
respect to `g`; `: !, _` drops the first and keeps the second. The seed `g` is
the lambda's argument, that is the previous value of the recursion.

Run it (`-n 1200 --every 200`). Both gains climb from 0 to `0.699` in about
1 000 samples (23 ms), and the third output — the difference between the
hand-written and the automatic derivative — is exactly `0` on every frame.
That is the whole promise of automatic differentiation: the derivative of your
program, exact, without writing it.

> **Why it converges.** The gradient points uphill on the loss; stepping
> against it goes downhill. With `lr = 0.01` and a noise of unit variance the
> effective time constant is about `1 / (lr * E[x^2])` ≈ 300 samples.

## 2. Reading `fad` and `rad`

Before using the library, look at what the primitives return. Two sliders,
one product:

```faust
x = hslider("x", 2.0, 0.0, 10.0, 0.01);
y = hslider("y", 3.0, 0.0, 10.0, 0.01);
process = fad(x * y, (x, y)), rad(x * y, (x, y));
```

Run with `-n 1`: the six outputs are `6, 3, 2, 6, 3, 2`.

- `fad(expr, (s0, s1))` gives each output of `expr` followed by its
  derivatives with respect to each seed: `[x*y, d/dx = y, d/dy = x]`.
- `rad(expr, (s0, s1))` gives all outputs of `expr`, then the gradients:
  the same numbers here, in a different layout.

The seeds are whatever signals you list; for a loss with `N` parameters, one
call gives the `N` derivatives. Everything in this tutorial uses `fad`; `rad`
comes back in section 10.

## 3. The same loop with the library

The library packages the loop of section 1 as `descend_1D`: you give it the
loss as a function of the parameter, an **engine** that turns a gradient into
a step, bounds, an initial value and a reset signal, and it returns the
learned parameter:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
x = no.noise;
target = 0.7 * x;
loss(g) = op.mse(g * x, target);
g = op.descend_1D(loss, op.adam_g(0.002, 0.9, 0.999, 1e-8), -4.0, 4.0, 0.0, 0.0);
process = g, target - g * x;
```

Run with `-n 3000 --every 500`: `g` reads `0.546, 0.685, 0.6998, 0.699999,
0.700000` at 500, 1 000, 1 500, 2 000, 2 500 samples, and the second output,
the residual, goes to zero with it.

Three things changed compared to section 1:

- `op.mse(y, t)` is the squared error; the library has other losses (section
  7);
- `op.adam_g(lr, 0.9, 0.999, 1e-8)` is **Adam**, the default engine of deep
  learning. Instead of stepping by `lr * gradient`, it keeps a running average
  of the gradient (momentum) and of its square, and steps by
  `lr * average / sqrt(average of squares)`: the step size is about `lr`
  whatever the gradient's scale. Where plain descent needs `lr` tuned to the
  units of the problem, Adam needs `lr` tuned to how fast you want to move —
  here 0.002 per sample;
- the last four arguments are the bounds `[-4, 4]`, the initial value `0`,
  and a reset signal (`0` here; a `button("reset")` works).

Engines are partially applied: `op.adam_g(0.002, 0.9, 0.999, 1e-8)` is a
function of one remaining argument, the gradient, which is what the loop calls
it with. The other gradient engines have the same shape:
`op.sgd_g(lr)`, `op.momentum_g(lr, 0.9)`, `op.rmsprop_g(lr, 0.999, 1e-8)`,
`op.lion_g(lr, 0.9, 0.99)`.

## 4. A filter: least squares and normalization

Now a parameter inside a recursion: the pole of a one-pole filter
`y[n] = x[n] + p y[n-1]`. Two things are new. The model has memory, so the
derivative of its output with respect to `p` depends on the whole past —
`fad` handles that by carrying the derivative along with the state, you do
not have to think about it. And we switch to the library's second family of
loops, `lsq_1D`, which differentiates the **model** rather than the loss:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
onepole(p, sig) = sig : + ~ *(p);
x = no.noise;
target = onepole(0.6, x);
p = op.lsq_1D(onepole, op.nlms(0.01, 1e-6, 0.99), -0.99, 0.99, 0.0, 0.0, target, x);
process = p, target - onepole(p, x);
```

`lsq_1D(mdl, engine, lo, hi, init, reset, target, x)` takes the model as a
function `mdl(p, x)` and the target and input as signals; the loss is the
squared error. Its engines receive two numbers instead of one: the residual
`r = model - target` and the **sensitivity** `j = d(model)/dp`. Keeping them
apart allows **NLMS**, normalized LMS: the step `mu * r * j` is divided by the
smoothed power of `j`, so it does not depend on how loud the input is.

Run with `-n 4000 --every 500`: `p` is `0.600013` at 500 samples and
`0.600000` from 2 000 on.

Why normalization matters: with a plain `op.lms(0.02)` step tuned for an input
of level 1, the same 3-tap FIR converges perfectly at level 1, is a hundred
times too slow at level 0.1 and hits its bounds at level 10; with
`op.nlms(0.02, 1e-6, 0.99)` it converges identically at all three levels.
Audio levels vary by 40 dB in a session; normalize.

## 5. Two parameters with different units

Identify a resonant low-pass: target `fi.resonlp(1200, 2.0)`, model
`fi.resonlp(f, q)`, started at `(1000, 1.0)`. The frequency is in hertz, the
quality factor has no unit. This is the moment beginners lose a day, so watch
what happens with a single learning rate.

### 5.1 One rate for both: the scale problem

Lion is an engine that steps by exactly `±lr` in the direction of its momentum
sign, whatever the gradient's magnitude — a good default when parameters have
different units, as long as `lr` makes sense for each of them:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
x = no.noise;
target = x : fi.resonlp(1200.0, 2.0, 1.0);
loss(f, q) = op.mse(x : fi.resonlp(f, q, 1.0), target);
lion = op.lion_g(0.001, 0.9, 0.99);
learned = op.descend_2D(loss, lion, lion, 20.0, 20000.0, 0.1, 10.0, 1000.0, 1.0, 0.0);
process = learned;
```

Run with `-n 100000 --every 10000`: `q` reaches 2.0, but `f` moves by 0.001 Hz
per sample — 1 Hz every 1 000 samples — and is still at 1 090 Hz after 100 000
samples. A step that is right for `q` is absurdly small for `f`. Two fixes
follow; both are worth knowing.

### 5.2 Fix one: learn in a domain where steps make sense

Learn `u = log(f)` instead of `f`. A step of 0.001 in `u` is a 0.1 % change of
frequency, which is the same kind of quantity as a step of 0.001 in `q`. The
model just applies `exp`:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
x = no.noise;
target = x : fi.resonlp(1200.0, 2.0, 1.0);
loss(u, q) = op.mse(x : fi.resonlp(exp(u), q, 1.0), target);
lion = op.lion_g(op.lr_exp(0.001, 0.00001, 20000.0), 0.9, 0.99);
learned = op.descend_2D(loss, lion, lion, log(20.0), log(20000.0), 0.1, 10.0, log(1000.0), 1.0, 0.0);
u = learned : _, !;
q = learned : !, _;
process = exp(u), q;
```

`op.lr_exp(0.001, 0.00001, 20000)` is a learning-rate **schedule**: it decays
exponentially from 0.001 towards 0.00001 with a time constant of 20 000
samples, so the search is fast at first and quiet at the end. Learning rates
are signals; a schedule is passed where a constant would be.

Run: `(1206, 2.003)` at 10 000 samples, then within about 2 % of
`(1200, 2.0)`. Good, with a residual jitter that Lion's fixed step size leaves.

### 5.3 Fix two: let the algorithm find the scales

`lm_2D` is a damped **Gauss-Newton** loop, the recursive form of the method
system identification has used for decades. It builds a 2x2 matrix from the
sensitivities of both parameters, which encodes how strongly each affects the
output and how they interact, and solves for the step. No per-parameter rate:
one gain `mu`, one damping `lambda`, one forgetting factor `a`:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
x = no.noise;
target = x : fi.resonlp(1200.0, 2.0, 1.0);
mdl(f, q, sig) = sig : fi.resonlp(f, q, 1.0);
process = op.lm_2D(mdl, 0.01, 0.1, 0.99, 20.0, 20000.0, 0.1, 10.0, 1000.0, 1.0, 0.0, target, x);
```

Run with `-n 30000 --every 5000`: `(1200.000000, 2.000000)` at 5 000 samples
and it stays there. Rule of thumb for its settings: `mu = 1 - a`,
`lambda = 0.1`, `a` between 0.99 and 0.999. `lm_3D` does the same for three
parameters.

## 6. Stability: the five-coefficient biquad

A biquad has three zeros coefficients `b0, b1, b2` and two pole coefficients
`a1, a2`. The poles are dangerous: outside the region `|a2| < 1`,
`|a1| < 1 + a2` the filter is unstable, and once it has blown up no optimizer
recovers. Bounding `a1` and `a2` to a rectangle is not enough, because the
stable region is a triangle. Try it: learn `(a1, a2)` directly with rectangular
bounds, starting from `(1.9, -0.5)` — inside the rectangle, outside the
triangle — and, side by side, learn two **reflection coefficients**
`k1, k2 in (-1, 1)` that the library maps to `a1 = k1 (1 + k2)`, `a2 = k2`.
That map covers exactly the triangle, so every point of the `(k1, k2)` box is a
stable filter:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
x = no.pink_noise;
target = fi.tf2(0.1, 0.2, 0.1, -1.0, 0.4, x);
adam = op.adam_g(0.001, 0.9, 0.999, 1e-8);
// (a) a1, a2 learned directly, rectangular bounds, started inside the bounds but outside the stability triangle
loss_raw(b0, b1, b2, a1, a2) = op.mse(fi.tf2(b0, b1, b2, a1, a2, x), target);
raw = op.descend_5D(loss_raw, adam, adam, adam, adam, adam,
                    -2, 2, -2, 2, -2, 2, -1.92, 1.92, -0.92, 0.92,
                    0, 0, 0, 1.9, -0.5, 0);
// (b) reflection coefficients: every point of the box is a stable filter
loss_k(b0, b1, b2, k1, k2) = op.mse(fi.tf2(b0, b1, b2, a1, a2, x), target)
with { a1 = op.poles_from_reflection(k1, k2) : _, !; a2 = op.poles_from_reflection(k1, k2) : !, _; };
kk = op.descend_5D(loss_k, adam, adam, adam, adam, adam,
                   -2, 2, -2, 2, -2, 2, -0.999, 0.999, -0.999, 0.999,
                   0, 0, 0, 0.95, -0.5, 0);
r4 = raw : !, !, !, _, !;
k1 = kk : !, !, !, _, !;
k2 = kk : !, !, !, !, _;
process = r4, op.poles_from_reflection(k1, k2);
```

Run with `-n 200000 --every 20000`: the raw `a1` (first output) is pinned at
its bound `1.92` from the first frames — the filter has blown up and the
gradient is garbage — while the reflection form (second and third outputs)
reaches `(-0.999, 0.3999)` for a target of `(-1.0, 0.4)` after 60 000
samples.

Note the two Faust idioms in `loss_k`: there is no destructuring, so the two
outputs of `poles_from_reflection` are projected with `: _, !` and `: !, _`;
and a five-output expression applied to a five-argument function is a partial
application, not a spread, so the coefficients are projected one by one.

The complete example, with a target the user controls, a reset button and a
single Lion rate on an exponential schedule, is section 4 of
[docs/fad-rad-synthesis-en.md](../docs/fad-rad-synthesis-en.md): the five
coefficients land within `1e-5` of the target after 300 000 samples.

## 7. The loss is yours

With `descend_ND` the loss is any Faust function of the parameters. Two
situations where the squared error is the wrong loss:

### 7.1 Outliers

Add impulsive spikes of `±20` every 97 samples to a target of amplitude 0.7,
and learn the gain through `mse` and through `logcosh`, a loss that is
quadratic near zero and linear far away, so its gradient is bounded:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
x = no.noise;
spike = float((ba.time % 97) == 0) * 20.0 * op.sgn(x');
target = 0.7 * x + spike;
g_mse = op.descend_1D(\(g).(op.mse(g * x, target)), op.sgd_g(0.01), -4, 4, 0, 0);
g_robust = op.descend_1D(\(g).(op.logcosh(g * x, target)), op.sgd_g(0.01), -4, 4, 0, 0);
process = g_mse, g_robust;
```

Run with `-n 40000 --every 5000`: the `mse` gain wanders between 0.49 and
0.88, kicked by every spike; the `logcosh` gain stays within 0.69–0.71.
`op.pseudo_huber(delta, y, t)` is the other robust loss, with an explicit
transition scale.

### 7.2 Matching a sound, not a waveform

Sample-by-sample error assumes the model and the target see the *same*
excitation. Often they do not: you want the model to sound like the target,
not to reproduce its waveform. Comparing smoothed powers ignores phase.
Here the target is a low-pass at 800 Hz on one noise, the model a low-pass on
an independent noise, the loss compares the log powers, and the cutoff is
learned in the log domain with plain SGD:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
excitation_a = no.noise;
excitation_b = no.noises(4, 1);   // an independent noise generator
target = excitation_a : fi.lowpass(1, 800.0);
loss(u) = op.log_energy_loss(0.999, 1e-9, excitation_b : fi.lowpass(1, exp(u)), target);
u = op.descend_1D(loss, op.sgd_g(0.00005), log(20.0), log(10000.0), log(3000.0), 0.0);
process = exp(u), exp(op.polyak(0.9999, u));
```

Run with `-n 400000 --every 50000`: the cutoff comes down from 3 000 Hz and
settles between 770 and 850 Hz — a jitter that comes from estimating a power
over about 1 000 samples of noise. `op.polyak(0.9999, u)` is a smoothed
readout of the parameter for the audible path. With `mse` on this pair of
signals the cutoff would stay stuck at the 20 Hz bound: there is nothing to
learn from a waveform that cannot be matched.

Two rules that this example teaches:

- a loss built on a smoothing (`energy_loss`, `log_energy_loss`) sees the
  world with a delay of about `1 / (1 - a)` samples; the optimizer must be
  slower than that or the loop oscillates — hence `lr = 5e-5` here;
- on a noisy loss, prefer SGD to Adam: SGD's step follows the gradient
  magnitude and dies out near the optimum, Adam's step is always about `lr`,
  which becomes a random walk.

## 8. Hygiene: schedules, gating, readout, reset

Real signals stop and start. An optimizer that keeps learning in silence
drifts on noise; one that never slows down jitters forever. This example puts
the tools together: the signal is present half of the time, a small
measurement noise is added, learning is gated on the input power, the learning
rate decays, the readout is averaged, and a button resets the parameter:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
present = float((ba.time % 20000) < 10000);        // signal present half of the time
x = no.noise * present;
target = 0.7 * x + 0.001 * no.noises(4, 2);       // plus measurement noise
loss(g) = op.mse(g * x, target);
vad = op.ema(0.99, x * x) > 0.001;                 // learn only when there is signal
lr = op.lr_exp(0.02, 0.001, 20000.0);
upd(grad) = op.sgd_g(lr, op.gate_g(vad, grad));
g = op.descend_1D(loss, upd, -4, 4, 0, button("reset"));
process = g, op.polyak(0.999, g), lr;
```

Run with `-n 80000 --every 10000`: `g` is at `0.69994` after 10 000 samples
and within `±5e-5` of 0.7 afterwards; the third output shows the learning rate
going from 0.02 to 0.0016. `upd` shows how conditioning composes: it is an
ordinary function of the gradient, built from library pieces, passed as the
engine.

## 9. Solving instead of learning: Newton

The same derivative machinery solves equations. Virtual-analog models are full
of implicit ones — the output of a saturating feedback loop depends on itself:
`y = tanh(x - fb * y)`. Newton's method finds `y` in a few steps, each needing
the residual `F(y) = y - tanh(x - fb y)` and its derivative `F'(y)`; one `fad`
gives both, and `op.newton(N, F, y0)` unrolls `N` steps:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
fb = 2.0;
// implicit saturator: y = tanh(x - fb * y), solved for y every sample
residual(x, y) = y - ma.tanh(x - fb * y);
solve(x) = op.newton(5, residual(x), 0.0);
process(x) = solve(x), residual(x, solve(x));
```

Run with `--in sine:220 -n 2000 --skip 1000 --quiet`: the first output is the
solved signal (peak 0.33 for a unit sine, the feedback compresses it), the
second is the residual of the solution — `0` to numerical precision on every
frame. This is the building block of zero-delay-feedback filters and diode
clippers.

## 10. Handing gradients to a host: `rad`

Everything so far learned inside the graph. Sometimes the program should only
*produce* derivatives, and a host (a plugin, a Python script, a test harness)
does the accumulation and the update — for example to train on a batch of
recordings rather than on the live signal. That is what `rad` is for:

```faust
gain = hslider("gain", 1.0, -4.0, 4.0, 0.001);
bias = hslider("bias", 0.0, -4.0, 4.0, 0.001);
process = rad(gain * _ + bias, (gain, bias));
```

Run with `--in sine:220 -n 5`: three outputs, `[gain * x + bias, x, 1]` — the
output and its two gradients. The host reads them, forms the loss gradient
(`2 * (out - target) * d/dgain`, summed over a block), and writes the sliders
back. [docs/rad-usage-en.md](../docs/rad-usage-en.md) has the full loop in
Rust, including an adaptive notch filter. One caveat: through delays and
recursions `rad` works block by block (the derivative is reset at the end of
each `compute` block), which is why the in-graph loops of this tutorial use
`fad`.

## 11. Learning at its own rate: `ondemand`

Everything so far ran once per sample: the model, the derivative, and the
update. Nothing forces the update to be that frequent. `faust-rs` has a
primitive, `ondemand`, that runs a sub-expression only when a clock fires and
holds its outputs in between:

```faust
(clock, inputs...) : ondemand(body)
```

`ondemand(body)` has one more input than `body`, the clock, first. Inside the
body, time is *fire time*: a `~` recursion advances once per firing, a delay
is one firing long. That is exactly what an optimizer wants when it should
step once per frame. `interleave.lib` (also in `libraries/`) provides the
frame clock, `il.frame_clock(N)`, which fires every `N` samples, and
`il.serialize_in(N)`, which turns a stream into the `N` parallel samples of
the current frame.

### 11.1 The whole optimizer, clocked

Put the loop of section 3 inside a block that fires every 64 samples. The
body receives the excitation and the target as inputs and closes the loss
over them:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
il = library("interleave.lib");
x = no.noise;
target = 0.7 * x;
learn(xi, ti) = op.descend_1D(\(g).(op.mse(g * xi, ti)), op.adam_g(0.02, 0.9, 0.999, 1e-8), -4.0, 4.0, 0.0, 0.0);
g = (il.frame_clock(64), x, target) : ondemand(learn);
process = g, target - g * x;
```

Run with `-n 20000 --every 2000`: `g` reads `0.630` at 4 000 samples, `0.7097`
at 6 000, `0.70005` at 12 000 and `0.7000 ± 1e-6` at the end — after 312
optimizer steps instead of 20 000. Between firings `g` is held, so the model
`g * x` at audio rate always sees a valid parameter. The `fad` graph, the
expensive part, runs 64 times less often; the price is that each step sees one
sample of the frame, hence `lr = 0.02` rather than `0.002`.

### 11.2 Gradient at audio rate, update per frame

A better use of the frame: compute the gradient on every sample, average it,
and let the block apply one step per frame. The block receives the previous
parameter and the averaged gradient as explicit inputs, and its held output is
fed back with `~`:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
il = library("interleave.lib");
N = 64;
x = no.noise;
target = 0.7 * x;
lr = 0.5;
g = learn ~ _
with {
    learn(gprev) = (il.frame_clock(N), gprev, gavg) : ondemand(step)
    with {
        grad = fad(op.mse(gprev * x, target), gprev) : !, _;   // audio rate
        gavg = op.ema(1.0 - 1.0 / N, grad);                     // averaged over ~N samples
        step(gp, ga) = op.clip(-4.0, 4.0, gp - lr * ga);        // once per frame
    };
};
process = g, target - g * x;
```

Run with `-n 20000 --every 2000`: `0.700000` from 4 000 samples on. Nothing is
lost by averaging: the frame-rate step sees the mean gradient of the frame,
which is what a batch step in a training framework sees. Note the shape: the
seed `gprev` and the `fad` are outside the block, the update is inside, and
the two communicate only through the block's inputs and its held output.

The library packages this pattern as `descend_1D_clocked` (and `2D` … `5D`):
same arguments as `descend_1D` with the clock first. It averages the gradient
with `op.frame_mean`, an exact mean over the frame reset by the clock, keeps
the parameter at `init` until the first firing, and latches a reset shorter
than a frame until the next firing:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
il = library("interleave.lib");
x = no.noise;
target = 0.7 * x;
loss(g) = op.mse(g * x, target);
g = op.descend_1D_clocked(il.frame_clock(64), loss, op.sgd_g(0.5), -4.0, 4.0, 0.0, 0.0);
process = g, target - g * x;
```

Run with `-n 20000 --every 2000`: `0.700000` from 4 000 samples on, like the
hand-written version. The engine runs in fire time, so an Adam or a schedule
given here counts frames, not samples: on the resonant filter of section 5,
`op.descend_2D_clocked(il.frame_clock(64), loss, adam, adam, ...)` with
`adam = op.adam_g(0.02, 0.9, 0.999, 1e-8)` on `(log f, q)` reaches
`(1200.2, 1.996)` after 10 000 samples and stays within 1 % of `(1200, 2.0)`
afterwards — Adam's fixed step leaves the usual small wobble, which a
schedule removes.

### 11.3 A spectral loss, one step per frame

This is the pattern that brings Faust closest to how DDSP papers train: a loss
on a *spectrum*, updated once per frame. The frame of `N = 8` samples enters
the block as eight inputs; inside, the loss scales the frame by `g`, takes an
FFT (`an.fft` from `analyzers.lib`), sums the magnitudes and compares the sum
with a target; `descend_1D` runs on that loss, in fire time:

```faust
il = library("interleave.lib");
an = library("analyzers.lib");
si = library("signals.lib");
no = library("noises.lib");
op = library("optimizers.lib");
N = 8;
target_energy = 4.0;
cmag(re, im) = sqrt(re * re + im * im + 0.000000001);
magsum = par(m, N, cmag) :> _;
// The block receives the N samples of the frame as named arguments (a frame
// operator with free `_` inputs would get its inputs duplicated at each use).
learn(x0, x1, x2, x3, x4, x5, x6, x7) =
    op.descend_1D(loss, op.adam_g(0.02, 0.9, 0.999, 1e-8), 0.01, 10.0, 1.0, 0.0)
with {
    // spectral loss of the frame scaled by g: (sum |X_k| - target)^2
    loss(g) = (x0, x1, x2, x3, x4, x5, x6, x7) : par(i, N, *(g) : (_, 0)) : an.fft(N) : magsum : -(target_energy) <: _ * _;
};
process = no.noise : il.serialize_in(N) : (il.frame_clock(N), si.bus(N)) : ondemand(learn);
```

Run with `-n 40000 --skip 20000 --quiet`: `g` averages `0.3405` over the second
half, with a frame-to-frame jitter of about `±0.03` — the spectrum of a random
frame varies, so the loss is noisy. The least-squares optimum for this noise
can be computed by hand: `0.340`. A magnitude with an epsilon under the square
root keeps the derivative defined at an empty bin.

The comment in the code is a rule to remember: a frame operator written with
free `_` inputs must not be passed around as an open expression, because every
use duplicates its inputs — the block's arity explodes into a "sequential
composition mismatch". Give the body named arguments.

A variant keeps the parameter outside the block and passes it in as an
explicit input; the block then outputs the held gradient, and the update
`g - lr * grad * clock` is gated by the clock at audio rate. It reaches the
same `0.34`. What does *not* work is capturing an outer audio-rate signal
inside the body without passing it as an input: keep the seed, the loss and
the update in the same domain, or connect them through the block's inputs.

Three last things about clock domains. `ma.SR` is not adapted inside
`ondemand` (its rate is unknown statically), so compute rate-dependent values
outside and pass them in. `rad` does not cross a domain boundary; the
in-graph patterns above are `fad` patterns. And the reference for the
primitives themselves, including `upsampling` and `downsampling`, is
[docs/ondemand-note-en.md](../docs/ondemand-note-en.md).

## 12. Where to go next

- **Adaptive effects.** Section 6 of
  [docs/fad-rad-synthesis-en.md](../docs/fad-rad-synthesis-en.md) is an
  active-noise-control loop (FxLMS) written with `fad`; the corpus file
  `tests/corpus/auto_wah_fad_host.dsp` is an auto-wah whose gradients are
  exposed to the host.
- **Spectral losses.** `tests/corpus/ondemand_fad_spectral_loss_008.dsp`
  differentiates a loss computed on an FFT frame, the per-frame counterpart of
  section 7.2.
- **Reverse mode and hosts.** [docs/rad-note-en.md](../docs/rad-note-en.md)
  for the algorithm, [docs/rad-usage-en.md](../docs/rad-usage-en.md) for the
  workflow.
- **The library itself.** Every function of
  [optimizers.lib](optimizers.lib) carries a `#### Test` example that is
  compiled by the test suite; they are the smallest working usage of each
  function.

## 13. Frequently hit walls

| Symptom | Likely cause | Fix |
|---|---|---|
| The parameter never moves | its derivative is zero: it passes through a button, a checkbox, an integer cast or comparison inside the model | keep the parameter path in floating-point arithmetic |
| It moves the wrong way | sign convention: with `r = model - target` the MSE gradient is `+2 r j`; the synthesis note uses `err = target - model` and `-err * j` | pick one convention |
| `NaN` after a while | `abs` (derivative `x/|x|`) or a filter that went unstable | smooth losses (`logcosh`, `pseudo_huber`), reflection coefficients for poles |
| One parameter converges, another crawls | different units under one learning rate | log domain, Adam/Lion, or `lm_2D` |
| The loop oscillates with an energy loss | the optimizer is faster than the loss's smoothing | lower `lr` below `1 - a` |
| Jitter at the end | fixed step size on a noisy gradient | `lr_exp`/`lr_cos`, `polyak`, or SGD instead of Adam |
| `(a, b) = f(...)` does not parse | Faust has no destructuring | `a = f(...) : _, !; b = f(...) : !, _;` |
| `mdl(opts)` has the wrong arity | a multi-output expression is one argument | project each output and pass them separately |
| Convergence in double but not in float | precision loss in recursive tangents | compile with `-double` |
| `sequential composition mismatch` around an `ondemand` block | a frame operator with free `_` inputs used several times | give the body named arguments, one per frame sample |
| A block ignores what happens outside | the body captures an outer signal instead of receiving it | pass outer signals as explicit inputs of the block |

## Glossary

- **Model**: the Faust expression whose parameters are learned.
- **Target**: the signal the model should produce.
- **Loss**: a scalar measure of the error at the current sample.
- **Gradient**: the derivative of the loss with respect to the parameters;
  **sensitivity** (`j`): the derivative of the model's output.
- **Seed**: the signal `fad` differentiates with respect to.
- **Tangent**: a derivative produced by forward-mode AD (`fad`).
- **Engine**: the function that turns a gradient (or a residual and a
  sensitivity) into a step.
- **Learning rate** (`lr`): the step size; a **schedule** makes it vary.
- **Residual** (`r`): `model - target`.
- **Reparameterization**: learning a transformed parameter (a log, a
  reflection coefficient) so that every value is admissible.
