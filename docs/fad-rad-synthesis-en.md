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
infinite-horizon gradients.

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
The tracked runtime test verifies convergence for this exact source.

## 10. Choosing FAD or RAD

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
- a reverse sweep is preferable as the parameter count grows.

Measure representative programs before assuming a performance advantage: the
current simplification and common-subexpression passes can make FAD and RAD
costs close on small graphs.

## 11. Practical Limits

These primitives do not turn Faust into a general deep-learning framework.
They are most useful for constrained, interpretable parametric DSPs.

Keep these points in mind:

- seeds must be explicit;
- seeds are recognized by Signal IR identity after lowering; an expression
  that is merely algebraically equivalent is not solved automatically;
- learned parameters usually need bounds, smoothing, normalization, or
  clipping;
- recursive filter coefficients must remain in stable regions;
- temporal RAD uses the current `compute(count)` block as its reverse horizon;
- FAD has dual rules for valid `ondemand`/`upsampling`/`downsampling` blocks,
  with an opaque clock; current integration tests concentrate on FAD inside and
  around `ondemand`. RAD across a clock-domain boundary is still rejected;
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

## See Also

- [fad-note-en.md](fad-note-en.md) — FAD surface and implementation.
- [rad-usage-en.md](rad-usage-en.md) — host-driven RAD workflows.
- [rad-note-en.md](rad-note-en.md) — RAD algorithm and rule table.
