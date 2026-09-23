---
title: "Synthesis Note: FAD and RAD Use Cases in faust-rs"
author: "OpenAI Codex"
date: "2026-07-22"
---

# FAD and RAD Use Cases in `faust-rs`

French version: [fad-rad-synthesis-fr.md](fad-rad-synthesis-fr.md) (same
content; keep both versions synchronized).

This document presents FAD and RAD from the perspective of a Faust user. It
assumes no prior knowledge of automatic differentiation.

In a conventional Faust program, a DSP maps audio inputs and controls to audio
outputs. With FAD and RAD, it can also produce derivatives: for example, "how
does the output change if this gain increases?" or "which way should this
coefficient move to reduce the error?"

Two primitives are available:

```faust
fad(expr, seeds)
rad(expr, seeds)
```

These are `faust-rs` extensions: the C++ Faust reference compiler used by this
project does not currently recognize `fad` or `rad`.

In the rest of this note:

- the **primal** is the ordinary value of `expr`;
- a **seed** is a variable with respect to which the expression is
  differentiated;
- a **tangent** is a derivative produced by FAD;
- a **gradient** is a derivative produced by RAD;
- a **loss** is a scalar signal to minimize, often `err * err`.

## 1. Reading the Outputs

### FAD: Local Forward-Mode Derivatives

If `expr` produces `M` signals and `seeds` produces `N`, FAD emits each primal
output followed by its `N` tangents:

```text
fad(expr, (s0, s1, ...)) =
    [p0, dp0/ds0, dp0/ds1, ...,
     p1, dp1/ds0, dp1/ds1, ...,
     ...]
```

The output arity is therefore `M * (1 + N)`. For a scalar expression this
reduces to `[expr, d(expr)/ds0, d(expr)/ds1, ...]`.

Example:

```faust
x = hslider("x", 1, 0, 10, 0.01);
y = hslider("y", 2, 0, 10, 0.01);
process = fad(x * y, (x, y));
```

Outputs:

```text
[x*y, y, x]
```

FAD is a good fit when the derivative must stay in the Faust graph: a
recursive update, Newton iteration, self-training filter, adaptive control,
and similar patterns.

### RAD: Gradients of a Loss or Sum of Outputs

For a scalar expression:

```text
rad(loss, (p0, p1, ...)) =
    [loss, d(loss)/d(p0), d(loss)/d(p1), ...]
```

For a multi-output expression, RAD returns the gradients of the sum of the
primal outputs. In practice, RAD is therefore often applied to an already
constructed scalar loss:

```faust
err = target - model;
loss = err * err;
process = rad(loss, (p0, p1));
```

RAD is useful when there is one scalar loss and several parameters to adjust.

For a feed-forward body, RAD performs a symbolic reverse sweep. For a body with
delays or recursion it uses `BlockReverseAD`: the primal runs forward over the
current `compute(count)` block, then the adjoint runs backward with zero terminal
adjoint state at the end of that block. Gradient outputs are consequently
per-sample contributions to sum over the block, not already reduced scalars or
infinite-horizon gradients. Consumed inside the graph -- an adaptation
recursion reading `rad(loss(p), p) : !, _` at the sample that produces it --
the sweep sees that sample only: through a recursion it returns the direct
term, the past state held fixed, where FAD carries the derivative through the
recursion; for a feed-forward body both agree (section 11).

## 2. Self-Training Gain with FAD

This illustrative example is grounded in the tracked regression case
[`fad_recursive_local_projection.dsp`](../tests/corpus/fad_recursive_local_projection.dsp).

Use case: a DSP learns an unknown gain by comparing its output with a target.
The gain estimate is stored in Faust recursion, and FAD computes the derivative
of the loss with respect to that estimate.

```faust
import("stdfaust.lib");

target_gain = hslider("gain", 0.5, 0, 1, 0.01);
input = 1.0;
true_value = input * target_gain;

learned_gain = loop ~ _
with {
    loop(prev_gain) = next_gain
    with {
        rate = 0.01;

        learned_value = input * prev_gain;
        loss = (true_value - learned_value) * (true_value - learned_value);

        grad = fad(loss, prev_gain) : !, _;
        next_gain = prev_gain - rate * grad;
    };
};

process = true_value, (learned_gain : hbargraph("learned_gain", 0, 1));
```

This example shows that:

- the learned variable can be Faust recursive state;
- the loss can be computed inside the DSP;
- FAD provides `d(loss)/d(prev_gain)`;
- gradient descent itself can be written in Faust.

This is the smallest useful in-graph learning example: no Python loop or
external runtime is required to update the parameter.

## 3. Two-Parameter Resonant-Filter Identification

This is an illustrative design example. The multi-seed contract and recursive
gradients are covered separately by
[`fad_multi_seed.dsp`](../tests/corpus/fad_multi_seed.dsp) and
[`fad_recursive_local_projection.dsp`](../tests/corpus/fad_recursive_local_projection.dsp).

Use case: a model filter learns to follow a target filter. Its learned
parameters are frequency `f` and quality factor `q`.

```faust
import("stdfaust.lib");

process = no.noise : train ~ (_, _) : (!, !, _);

train(f_prev, q_prev, input) = f_next, q_next, model_out
with {
    f = select2(f_prev == 0.0, f_prev, 1000.0);
    q = select2(q_prev == 0.0, q_prev, 1.0);

    target_f = hslider("Target Freq", 1200, 20, 20000, 1);
    target_q = hslider("Target Q", 2.0, 0.1, 10.0, 0.01);

    target = input : fi.resonlp(target_f, target_q, 1.0);
    model_out = input : fi.resonlp(f, q, 1.0);
    err = target - model_out;

    diffs = fad(input : fi.resonlp(f, q, 1.0), (f, q)) : !, _, _;
    df = diffs : _, !;
    dq = diffs : !, _;

    raw_grad_f = -err * df;
    raw_grad_q = -err * dq;

    grad_f = max(-1.0, min(1.0, raw_grad_f));
    grad_q = max(-0.1, min(0.1, raw_grad_q));

    m_f = grad_f : si.smooth(0.9);
    v_f = (grad_f * grad_f) : si.smooth(0.999);
    m_q = grad_q : si.smooth(0.9);
    v_q = (grad_q * grad_q) : si.smooth(0.999);

    f_next = max(20.0, min(20000.0, f - 2.0 * (m_f / (sqrt(v_f) + 1e-3))))
        : hbargraph("Learned Freq", 20, 20000);

    q_next = max(0.1, min(10.0, q - 0.01 * (m_q / (sqrt(v_q) + 1e-3))))
        : hbargraph("Learned Q", 0.1, 10.0);
};
```

This example shows that:

- `fad(..., (f, q))` provides two sensitivities in one call;
- gradients can be smoothed, bounded, and normalized like DSP signals;
- an Adam/RMSProp-like optimizer can remain entirely in Faust;
- physical or numerical constraints are applied directly in the update.

This pattern is useful for system identification: define an interpretable
model, observe a target, and let the DSP adjust its parameters to reduce error.

## 4. Self-Training Five-Coefficient Biquad

This executable design example complements the adaptive biquad with RAD in
[`rad_tbptt_biquad1.dsp`](../tests/corpus/rad_tbptt_biquad1.dsp). The
project-local [`optimizers.lib`](../libraries/optimizers.lib) library used below
(prefix `op`) is versioned with `faust-rs`; the corpus fixture
[`opt_descend_lion_biquad_reflection.dsp`](../tests/corpus/opt_descend_lion_biquad_reflection.dsp)
is the library-free version checked by the test suite. Concatenate the Faust
blocks in this section and compile the resulting program with `-I libraries`.

Use case: learn the five coefficients `b0, b1, b2, a1, a2` of a biquad to
imitate a user-controlled target.

The audio model is compact:

```faust
import("stdfaust.lib");
op = library("optimizers.lib");

biquad_model(b0, b1, b2, a1, a2, audio) =
    fi.tf2(b0, b1, b2, a1, a2, audio);
```

Target parameters can be exposed as sliders:

```faust
t_b0 = vslider("[1] Target b0", 0.1, -2.0, 2.0, 0.001) : si.smooth(0.99);
t_b1 = vslider("[2] Target b1", 0.2, -2.0, 2.0, 0.001) : si.smooth(0.99);
t_b2 = vslider("[3] Target b2", 0.1, -2.0, 2.0, 0.001) : si.smooth(0.99);
t_a1 = vslider("[4] Target a1", -1.0, -1.90, 1.90, 0.001) : si.smooth(0.99);
t_a2 = vslider("[5] Target a2", 0.4, -0.90, 0.90, 0.001) : si.smooth(0.99);
```

The learned model does not expose `a1, a2` directly: it learns two reflection
coefficients `k1, k2` in `(-1, 1)` and maps them with
`a1 = k1 (1 + k2)`, `a2 = k2`. This map is a bijection onto the stability
triangle `|a2| < 1`, `|a1| < 1 + a2`, so every intermediate filter is stable,
which rectangular bounds on `a1, a2` cannot guarantee. The loss is written as
an ordinary Faust function of the five parameters, closed over the excitation
and the target:

```faust
noise = no.pink_noise;
target = biquad_model(t_b0, t_b1, t_b2, t_a1, t_a2, noise);

learned_model(b0, b1, b2, k1, k2) = biquad_model(b0, b1, b2, a1, a2, noise)
with {
    a1 = op.poles_from_reflection(k1, k2) : _, !;
    a2 = op.poles_from_reflection(k1, k2) : !, _;
};
loss(b0, b1, b2, k1, k2) = op.mse(learned_model(b0, b1, b2, k1, k2), target);
```

The core is a five-dimensional descent on that loss. One Lion engine, on an
exponentially decaying learning rate, serves the five parameters: Lion steps
by `±lr` in the direction of the sign of its momentum, whatever the scale of
each gradient, so zeros and poles need no separate rates:

```faust
lion = op.lion_g(op.lr_exp(0.0002, 0.000002, 30000.0), 0.9, 0.99);
reset = button("[6] Reset");

opts = op.descend_5D(
    loss,
    lion, lion, lion, lion, lion,
    -2.0, 2.0,
    -2.0, 2.0,
    -2.0, 2.0,
    -0.999, 0.999,
    -0.999, 0.999,
    0.0, 0.0, 0.0, 0.0, 0.0,
    reset
);
```

Extract the five learned parameters, map the reflection coefficients back to
poles, rebuild the learned model, and expose a complete `process`:

```faust
b0 = opts : _, !, !, !, !;
b1 = opts : !, _, !, !, !;
b2 = opts : !, !, _, !, !;
k1 = opts : !, !, !, _, !;
k2 = opts : !, !, !, !, _;
a1 = op.poles_from_reflection(k1, k2) : _, !;
a2 = op.poles_from_reflection(k1, k2) : !, _;

model = learned_model(b0, b1, b2, k1, k2);

process = target, model, b0, b1, b2, a1, a2;
```

`descend_5D` factors out this pattern:

```faust
grads(p1, p2, p3, p4, p5) =
    fad(loss(p1, p2, p3, p4, p5), (p1, p2, p3, p4, p5)) : !, _, _, _, _, _;
```

one `fad` call over the loss, whose five tangent lanes are the gradients
`dloss/dp1 ... dloss/dp5`, then one update engine per parameter and a
projection on the bounds. Rendered with `faustprobe` in double precision, the
five coefficients are within `1e-5` of the target after 300 000 samples.

This example shows that FAD is not limited to one slider, Faust libraries can
factor out optimizers, the loss can be any Faust expression (a robust or an
energy loss would plug in the same way), and IIR stability is best obtained
by reparameterization rather than by bounds. The original form of this
example, `optimize_5D` with `rmsprop` engines and rectangular pole bounds,
still compiles and converges with version 0.5.0 of the library.

## 5. Newton Iteration for an Implicit Equation

This illustrates composition of the two outputs of a single-seed FAD.

Use case: solve an analog-style implicit equation for `y`:

```text
E(y) = y - tanh(x - fb*y) = 0
```

Newton's method needs `E(y)` and `E'(y)`. FAD computes the derivative of the
error with respect to the current estimate `y`.

```faust
import("stdfaust.lib");

circuit_error(x, fb, y) = y - ma.tanh(x - fb * y);

newton_step(x, fb, y) = y - (err / den)
with {
    err = circuit_error(x, fb, y);
    den = fad(circuit_error(x, fb, y), y) : !, _;
};

solve_circuit(x, fb) = 0.0 : seq(i, 5, newton_step(x, fb));

process = _ <: _, solve_circuit(feedback)
with {
    feedback = hslider("Analog_Feedback", 2.0, 0.0, 10.0, 0.01);
};
```

FAD avoids a hand-written nonlinear derivative, the solver remains pure Faust,
and several Newton steps can be unrolled with `seq`. The pattern is relevant to
analog models, feedback saturation, circuit approximations, and zero-delay
solvers.

## 6. Active Noise Control with FxLMS

This executable example uses a one-coefficient controller and a first-order
secondary-path model.

Use case: adapt a control coefficient to minimize residual noise measured after
a secondary path. This is the classic FxLMS structure, with the derivative
obtained from FAD.

```faust
import("stdfaust.lib");

clamp(lo, hi, x) = min(hi, max(lo, x));
secondaryPath(x) = fi.lowpass(1, 1200, x);

process(ref, dist) = (loop ~ _) : !, _, _, _
with {
    mu = hslider("Mu", 0.001, 0.000001, 0.05, 0.000001);
    reset = button("Reset");
    filtered_ref = secondaryPath(ref);

    loop(w_prev) = w_next, err, y, w_prev
    with {
        y = w_prev * ref;
        err = dist + secondaryPath(y);

        sensitivity = fad(w_prev * filtered_ref, w_prev) : !, _;
        grad_w = 2.0 * err * sensitivity;

        updated = clamp(-2.0, 2.0, w_prev - mu * grad_w);
        w_next = select2(reset, updated, 0.0);
    };
};
```

The physical secondary path and the canonical learning recursion stay outside
the differentiated expression. FAD computes the controller sensitivity from
the filtered reference; the measured error completes the FxLMS gradient. The
adaptive coefficient is constrained, and the Reset button restarts learning.

## 7. Host-Driven Gain-and-Bias Regression with RAD

Source:
[`rad_gain_bias_train.dsp`](../tests/corpus/rad_gain_bias_train.dsp).

Use case: the DSP computes derivatives while the host accumulates the gradient
over a block and updates sliders between `compute` calls.

```faust
gain = hslider("gain", 1.0, -4.0, 4.0, 0.001);
bias = hslider("bias", 0.0, -4.0, 4.0, 0.001);

process = rad(gain * _ + bias, (gain, bias));
```

Outputs:

```text
[out, d(out)/d(gain), d(out)/d(bias)]
```

For a host-side MSE loss:

```text
err[n] = out[n] - target[n]
grad_gain = sum_n 2 * err[n] * d(out[n])/d(gain)
grad_bias = sum_n 2 * err[n] * d(out[n])/d(bias)
```

The DSP remains stateless from the optimizer's perspective, while the host can
choose batching, learning rate, clipping, optimizer, and update policy. This
fits plugins, offline tests, and application-driven training.

## 8. Adaptive Notch with RAD

Source:
[`rad_adaptive_notch_omega.dsp`](../tests/corpus/rad_adaptive_notch_omega.dsp).

Use case: identify the dominant frequency in a signal and move a notch toward
that frequency.

```faust
omega = hslider("omega", 1.0, 0.01, 3.0, 0.0001);

notch(xn, xn1, xn2) = xn - 2.0 * cos(omega) * xn1 + xn2;
process = rad(notch, omega);
```

The filter is:

```text
H(z) = 1 - 2*cos(omega)*z^-1 + z^-2
```

It places two zeros on the unit circle at angle `omega`. If the host minimizes
`loss = mean(y*y)`, gradient descent pushes `omega` toward the strongest input
frequency. The host can manage `x[n-1]` and `x[n-2]`, and the DSP exposes
`d(y)/d(omega)` for an external LMS update.

## 9. Multi-Tap FIR LMS with RAD

Source:
[`rad_tbptt_lms_fir3.dsp`](../tests/corpus/rad_tbptt_lms_fir3.dsp).

Use case: learn the coefficients of a three-tap FIR that imitates a hidden
target.

```faust
h0_star = 0.5;
h1_star = 0.3;
h2_star = -0.2;
lr = 0.02;

noise = lcg * 4.656612873077393e-10
with { lcg = +(12345) ~ *(1103515245); };

x  = noise;
x1 = x  : mem;
x2 = x1 : mem;
y_target = h0_star * x + h1_star * x1 + h2_star * x2;

taps = loop ~ (_, _, _)
with {
    loop(h0, h1, h2) = h0n, h1n, h2n
    with {
        y_pred = h0 * x + h1 * x1 + h2 * x2;
        err = y_target - y_pred;
        loss = err * err;

        g0 = rad(loss, h0) : !, _;
        g1 = rad(loss, h1) : !, _;
        g2 = rad(loss, h2) : !, _;

        h0n = max(-4.0, min(4.0, h0 - lr * g0));
        h1n = max(-4.0, min(4.0, h1 - lr * g1));
        h2n = max(-4.0, min(4.0, h2 - lr * g2));
    };
};

h0 = taps : _, !, !;
h1 = taps : !, _, !;
h2 = taps : !, !, _;

process = (y_target - (h0 * x + h1 * x1 + h2 * x2)) <: _, _;
```

RAD can be consumed inside a Faust adaptation loop, each coefficient receives
its loss gradient, and the residual lets the user hear convergence directly.
The tracked runtime test verifies convergence for this exact source. The body
is feed-forward (the delayed inputs do not depend on the coefficients), so
the one-sample horizon of an in-graph sweep loses nothing here: the gradient
is the exact one. `optimizers.lib` packages this pattern for `N` taps as
`descend_N_rad` and `lsq_N_rad`, one sweep per sample for the `N`
derivatives.

## 10. Learning at Frame Rate with `ondemand`

Executable examples; the clocked loops are in
[`optimizers.lib`](../libraries/optimizers.lib) 0.6.0 and the frame harness in
[`interleave.lib`](../libraries/interleave.lib). Compile with `-I libraries`.

Use case: adapt the parameters once per frame rather than once per sample —
to save CPU, to step on a mini-batch gradient, or because the loss lives on a
spectrum. `ondemand(C)` runs `C` only on the samples where its first input, the
clock, is non-zero, and holds the outputs in between; inside the body a
recursion advances once per firing. `fad` composes with it: differentiation
inside a block is supported (checked against finite differences), a seed may
enter the block as an explicit input, and a derivative never crosses a clock
boundary on its own (`rad` across a boundary is rejected).

The library form keeps the loss and `fad` at audio rate, averages the gradient
over the frame with `op.frame_mean` (an exact mean, reset by the clock), and
takes the step inside an `ondemand` block, so the engine's state advances once
per frame:

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

Rendered with `faustprobe`, the gain is `0.700000` from 4 000 samples on, with
one SGD step per 64-sample frame. Placing a whole loop inside a block,
`(il.frame_clock(64), x, target) : ondemand(learn)` with
`learn(xi, ti) = op.descend_1D(\(g).(op.mse(g * xi, ti)), ...)`, also works:
everything then runs in fire time on the block's inputs (gain within `1e-6`
of the target after 312 steps).

The frame-rate DDSP form: the frame of `N` samples enters the block as
inputs, the loss is computed on its FFT, and the optimizer steps once per
frame inside the block:

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

The learned gain settles at `0.34`; the least-squares optimum for this
excitation is `0.340`. Two rules the examples rely on: a body receives outer
signals as explicit inputs (a definition referenced inside is instantiated
again in the body's own time, not the outer signal), and a frame operator
with free `_` inputs is given named arguments, or its inputs are duplicated
at every use. `ma.SR` is not adapted inside `ondemand`. The primitives are described in
[ondemand-note-en.md](ondemand-note-en.md); a step-by-step walk-through is
section 11 of
[optimizers-ddsp-tutorial-en.md](../libraries/optimizers-ddsp-tutorial-en.md).

## 11. Choosing FAD or RAD

Use **FAD** when:

- the gradient must be consumed immediately in the DSP;
- the number of parameters is small or moderate;
- the patch contains a recursive update written in Faust;
- a local slope is needed, for example in Newton iteration or nonlinear DSP;
- a Faust optimizer library is being built.

Use **RAD** when:

- the starting point is a scalar loss;
- several gradients are needed for one loss;
- the host can accumulate contributions over a block;
- the task is regression, LMS, adaptive notch filtering, or externally driven
  parametric training;
- a reverse sweep is preferable as the parameter count grows;
- many parameters share one loss inside the graph: the `_rad` bus loops of
  `optimizers.lib` run one sweep per sample, exact for a feed-forward model
  and the direct term (past state held fixed) through a recursion.

Measure representative programs before assuming a performance advantage. On
a 16-tap FIR learned in the graph, the `rad` bus loop compiles to 1 182
interpreter instructions against 3 777 for `fad` (0.04 s against 0.10 s for
200 000 samples), 4 129 against 28 891 at 64 taps (0.13 s against 1.32 s);
on small graphs the simplification and common-subexpression passes can make
the two costs close.

## 12. Practical Limits

These primitives do not turn Faust into a general deep-learning framework.
They are most useful for constrained, interpretable parametric DSPs.

Keep these points in mind:

- seeds must be explicit;
- seeds are recognized by Signal IR identity after lowering; an expression
  that is merely algebraically equivalent is not solved automatically;
- learned parameters usually need bounds, smoothing, normalization, or
  clipping;
- recursive filter coefficients must remain in stable regions;
- temporal RAD uses the current `compute(count)` block as its reverse horizon
  for public outputs, and the current sample when the gradient is consumed
  inside the graph (the direct term through a recursion);
- FAD has dual rules for valid `ondemand`/`upsampling`/`downsampling` blocks,
  with an opaque clock; current integration tests concentrate on FAD inside and
  around `ondemand` (section 10). RAD across a clock-domain boundary is still
  rejected; a `rad` whose expression and seeds live inside one `ondemand`
  body runs in that domain (a block's inputs and foreign constants are
  leaves of the sweep, the clocked wrapper is passed through);
- the symbolic rules do not cover every signal family: FAD preserves the
  primal and uses zero tangents at unmodeled boundaries, while RAD rejects hard
  unsupported families such as mutable tables, soundfiles, and unrecognized
  foreign functions;
- read-only table lookup uses a symmetric finite-difference slope, not an
  analytic derivative of the table contents;
- non-smooth formulas are not regularized automatically; the current `abs`
  derivative may produce `NaN` at zero;
- large experimental patches should be reduced to small, testable cases before
  being treated as reference examples.

A good `faust-rs` differentiable patch starts with a clear audio model, a clear
scalar loss, and a small number of parameters, then adds constraints,
smoothing, and monitoring incrementally.

## 13. The Two Examples of the AES 2025 Paper

"Faust Autodiff: Towards Audio Domain-Specific Machine Learning" (T. Rushton,
AES AIMLA 2025) differentiates Faust programs at the source level: dual
signals `<s, grad s>` and one rule per composition operator, implemented by
pattern matching in a Faust library. Its two examples are in the corpus with
the paper's own routing, the parameters as slider seeds:

- [`fad_neuron_sigmoid.dsp`](../tests/corpus/fad_neuron_sigmoid.dsp) and
  [`rad_neuron_sigmoid.dsp`](../tests/corpus/rad_neuron_sigmoid.dsp), the
  neuron `y = sigmoid(w . x + b)` of its listing 4, with `ro.interleave` (a
  `route`) in the dot product;
- [`fad_iir_transposed.dsp`](../tests/corpus/fad_iir_transposed.dsp) and
  [`rad_iir_transposed.dsp`](../tests/corpus/rad_iir_transposed.dsp), the
  order-2 IIR of its listing 5, its subtraction re-routed with `_` and `!`.

The paper's limitations do not apply here, because `fad` and `rad` work on
the signal graph after propagation: `route`, the unapplied `ma.sub` of
`fi.iir` and the widgets have disappeared. `fad(fi.iir(bv, av), seeds)`
gives the fixture's tangents to the bit. The tests are in
[crates/compiler/tests/aes_autodiff_paper.rs](../crates/compiler/tests/aes_autodiff_paper.rs):
closed form for the neuron, finite differences on every frame for the IIR
under `fad`, block totals for the IIR under `rad`.

## 14. How Large a FIR or IIR Compiles

Measured on 2026-09-23 with `faustprobe --double --block 256 -n 48000 --time`
(Cranelift, one Apple laptop), the coefficients as slider seeds, the whole
bundle of primal and derivative lanes as outputs. Gradients were checked
against finite differences on lanes picked at random up to order 256
(`--fd-check` with an explicit `--grad-lane`, since `--train` assigns the
lanes in the order of its list).

FIR of N taps (`fi.fir`, N seeds):

| N | fad compile | fad compute | rad compile | rad compute | memory |
|---|---|---|---|---|---|
| 64 | 24 ms | 428x real time | 30 ms | 61x | 25 MB |
| 256 | 0.2 s | 37x | 0.2 s | 9.6x | 70 MB |
| 1024 | 5.6 s | 6x | 5.4 s | 1.4x | 0.6 to 0.7 GB |
| 4096 | 281 s | 1.1x | 265 s | 0.16x | 5 to 9 GB |

Direct-form IIR of order N (`fi.iir`, 2N + 1 seeds):

| N | fad compile | fad compute | rad compile | rad compute |
|---|---|---|---|---|
| 16 | 69 ms | 103x | 19 ms | 131x |
| 64 | 2.5 s | 1.4x | 0.13 s | 16x |
| 256 | 198 s | 0.05x | 0.8 s | 4.3x |

Cascade of K biquads (`fi.tf2`, 5K seeds):

| K | fad compile | fad compute | rad compile | rad compute |
|---|---|---|---|---|
| 16 | 1.05 s | 3.5x | 42 ms | 64x |
| 64 | 189 s | 0.11x | 0.21 s | 10x |
| 256 | given up after 900 s | | 2.6 s | 2.1x |

What follows:

- **FIR: a few hundred taps comfortably, a thousand for 5 s of
  compilation, 4096 is the practical limit** (minutes and gigabytes, still
  real time under `fad`). The cost is quadratic: each derivative lane is
  itself an N-tap FIR, so the graph has N^2 nodes. Beyond that the FIR would
  have to be a coefficient table with a loop, which neither Faust nor the AD
  rules have today.
- **IIR: under `rad`, order 256 or 256 sections compile in under 3 s and
  run in real time.** The block reverse sweep is linear in the size of the
  body. Under `fad` on a recursion each tangent is a full recursion:
  quadratic, so order 64 or 64 sections is the reasonable ceiling (2 to 3
  minutes, 5 GB), and 256 does not compile.
- **Rule of thumb**: `fad` for small graphs and in-graph learning, `rad`
  as soon as there is a high-order recursion or more than a hundred or so
  parameters.

Against a frequency-sampling library such as FLAMO (Dal Santo et al., 2025):
there a 4096-tap FIR is a product of frequency responses, free in gradient,
and a 6x6 FDN differentiates through a matrix inverse; the sizes above are
those of the time domain, sample by sample, with the exact recursion. Their
cases (a 6-line FDN with an orthogonal matrix, a GEQ per line, an
active-acoustics FIR of a few thousand taps) are reachable under `rad`
except the last, where thousands of taps per channel over several channels
exceed what compiles. Two differences of substance: the time aliasing they
must mitigate does not exist here, and their spectral loss remains to be
written on our side. A lane of a tap or a pole further than the block is
zero under `rad` (h1023 with a 256-frame block): the documented horizon.

### Against the DDSP literature

Time-domain differentiable IIRs in the literature stay at orders 1 to 6.
Kuznetsov, Parker and Esqueda (DAFx 2020) train first- and second-order
sections, a state-space filter of order 2 and 6 and three biquads in
series, by truncated backpropagation through time over 2048-sample
sequences with PyTorch's autograd. Yu et al. (DAFx 2024, torchlpc) write
the backward pass of an all-pole filter as a reverse filtering: orders 1
(compressor), 2 (TB-303), 6 (phaser); one optimisation step on the TB-303
takes 29 to 32 ms in the time domain against 57 to 1795 ms by frequency
sampling depending on the window (batch of 34 notes, M1 Pro), a training
17 minutes against 43. Yu and Fazekas (arXiv 2511.14390, philtorch) give
the general state-space form with analytic gradients as a C++/CUDA kernel,
measured at order 2 only, "higher orders being cascades of sections": on
an i7-7700K, one thread, 2^20 samples (65 s at 16 kHz) take about 10 ms
forward and as much backward, some 6000x real time per pass; naive
autograd is "at least 1000x" slower and frequency sampling in between.

Our measurements on the same kind of object, 4 biquads under `rad` at 576x
real time with the primal and its 20 gradient lanes in one pass, an order-4
IIR at 1585x, are in the class of those dedicated kernels (a factor of a
few, the whole bundle emitted at once) and three orders of magnitude above
the DDSP practice of autograd. The structural difference: one kernel per
filter form there, one compiler for any body here. Nobody trains a
direct-form IIR of order 64 or 256 in the time domain; high orders go
through frequency sampling (Nercessian 2020 for equaliser biquad cascades,
FLAMO, the Aalto FDNs), where `rad`, linear in the body, compiles them in
under 3 s.

FIRs in the literature never go through the time domain beyond a few
dozen taps. DDSP (Engel 2020) filters by frequency sampling, 65 magnitudes
per frame, a 257-point Hann window, hop 256, and convolves reverberation
responses of 10 000 to 100 000 samples by FFT, direct convolution being
"intractable". Differentiable active acoustics (De Bortoli, DAFx 2024)
learns matrices of 2x2 to 13x4 FIRs of order 100 and 1000 sampled on
480 000 frequency points, batches of 2400 points, 10 epochs. GRAFX (Lee,
DAFx 2024) has a zero-phase 2047-tap FIR equaliser from the IFFT of 1024
log-magnitudes, FFT convolution, and renders graphs of 350 to 400
processors at 25 to 100 graphs per second on an RTX 3090 with 5 sources
of 2^17 samples. The colorless FDN (Dal Santo, DAFx 2023) has 4, 6 or 8
lines, 6000 to 9000 modes, on 480 000 frequency points; RIR2FDN (2024),
6 lines, 5.7 to 41.9 s per iteration on a V100 for about 1000 iterations.
Our 4096-tap limit by expansion of the time-domain graph covers one
active-acoustics filter (order 1000) but not its full matrix (52 filters
of 1000 taps), and is far from DDSP's reverberation responses, which are
differentiable only because the FFT makes the gradient free: a long FIR
needs a table-plus-loop representation or an FFT path before it competes.

| | time-domain literature | frequency-sampling literature | faust-rs |
|---|---|---|---|
| low-order IIR | our class of speed, dedicated kernels | slower, aliasing | compiled, exact |
| high-order IIR | absent | the only way, with aliasing | `rad` linear, order 256 |
| long FIR | absent | FFT, free | 4096 taps at most |
| batches, GPU | yes | yes | not measured |
| spectral loss | rare | native | to be written |

The literature confirms two things: exact time domain beats frequency
sampling as soon as it is compiled (Yu and Fazekas's conclusion is ours),
and frequency sampling keeps the long FIR and the large FDN. Our
weaknesses are not the speed per sample but the absence of batching and
of a spectral loss, and the long FIR.

Sources: [Kuznetsov et al. 2020](https://www.dafx.de/paper-archive/2020/proceedings/papers/DAFx2020_paper_52.pdf),
[Yu et al. 2024](https://arxiv.org/abs/2404.07970),
[Yu and Fazekas 2025](https://arxiv.org/abs/2511.14390),
[Engel et al. 2020](https://arxiv.org/abs/2001.04643),
[De Bortoli et al. 2024](https://www.dafx.de/paper-archive/2024/papers/DAFx24_paper_64.pdf),
[Lee et al. 2024](https://arxiv.org/abs/2408.03204),
[Dal Santo et al. 2023](https://www.dafx.de/paper-archive/2023/DAFx23_paper_32.pdf),
[Dal Santo et al. 2024](https://arxiv.org/abs/2404.00082),
[FLAMO](https://arxiv.org/abs/2409.08723).

## See Also

- [fad-note-en.md](fad-note-en.md) — FAD surface and implementation.
- [rad-usage-en.md](rad-usage-en.md) — host-driven RAD workflows.
- [rad-note-en.md](rad-note-en.md) — RAD algorithm and rule table.
- [ondemand-note-en.md](ondemand-note-en.md) — the clock-domain primitives.
- [optimizers-overview-en.md](../libraries/optimizers-overview-en.md) and
  [optimizers-ddsp-tutorial-en.md](../libraries/optimizers-ddsp-tutorial-en.md)
  — the optimizer library explained for newcomers, and a step-by-step
  tutorial.
