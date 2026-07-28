# DDSP Library & Self-Contained Training for faust-rs

**Date:** 2026-04-14
**Prerequisite:** Forward AD (`fad()`) implemented at compiler level in faust-rs, including FAD inside recursions (`suppress_fad` mechanism).
**Reference:** [hatchjaw/faust-ddsp](https://github.com/hatchjaw/faust-ddsp) — pure-Faust forward-mode AD library with SGD/Adam/RMSProp, loss functions, and backpropagation.

---

## Context

faust-rs already has `fad(expr)` which automatically differentiates an expression w.r.t. all its UI controls and produces expanded outputs `[primal, tangent_1, ..., tangent_K]`. This is a **compiler-level** transformation — much simpler for the user than the manual dual-signal routing of faust-ddsp's `diff.lib`.

What's missing is the **training infrastructure**: loss functions, optimizers, and the feedback loop that turns gradients into parameter updates. The faust-ddsp project proves this is feasible and useful in real-time audio contexts.

### Comparison of approaches

| | faust-ddsp (`diff.lib`) | faust-rs (`fad()`) |
|---|---|---|
| AD mechanism | Library: user rewrites each primitive as `d.diff(*)` | Compiler: `fad(expr)` — automatic, transparent |
| Variable declaration | `df.vars((x,y))` + `df.env()` | Automatic detection of hslider/vslider/numentry |
| Gradient access | Parallel signals alongside primals | Extra output signals after primals |
| Training loop | `df.backprop()` with `~` feedback | Not yet available |
| Optimizers | SGD, Adam, RMSProp in Faust | Not yet available |
| User complexity | High (manual routing of duals) | Low (just wrap with `fad()`) |

---

## Phase 2 — DDSP Library (`ddsp.lib`)

### Goal

Provide a Faust library that works with `fad()` outputs to enable host-driven and semi-automated gradient descent training. The user writes `fad(expr)` and the library handles loss computation, gradient scaling, and optimizer logic.

### 2.1 Loss Functions

Pure Faust functions that take a predicted signal and a ground-truth signal and produce a scalar loss. These operate on the **primal** output of `fad()`, not on tangents.

```faust
// ddsp.lib — Loss functions

// Mean Squared Error over a sliding window
// L = mean((pred - truth)^2)
// dL/dpred = 2 * (pred - truth) (used to scale tangents)
mse(windowSize) = ro.cross(2) : - : ^(2) : ba.slidingMean(windowSize);

// Mean Absolute Error
// L = mean(|pred - truth|)
// dL/dpred = sign(pred - truth)
mae(windowSize) = ro.cross(2) : - : abs : ba.slidingMean(windowSize);

// Huber loss: MSE when |error| < delta, MAE otherwise
huber(delta, windowSize) = ro.cross(2) : -
    : (_ <: (abs : <(delta)), _)
    : select2(_, ^(2) : *(0.5), abs : *(delta) : -(delta*delta*0.5))
    : ba.slidingMean(windowSize);
```

### 2.2 Gradient Scalers

Functions that combine the loss derivative with the FAD tangent outputs to produce the actual parameter gradients.

For MSE: `gradient_i = dL/dpred * dpred/dctrl_i = 2*(pred-truth) * tangent_i`

```faust
// Scale FAD tangent outputs by the loss derivative
// Inputs: truth, pred, tangent_1, ..., tangent_K
// Outputs: loss, grad_1, ..., grad_K
mse_gradients(K, windowSize) =
    route(K+2, K+3,
        (1,1), (2,2),         // truth, pred → loss
        (1,K+3),              // truth → error computation
        (2,K+3+1),            // pred → error computation  
        par(i, K, (i+3, i+3)) // tangents pass through
    )
    : mse(windowSize), error_derivative, si.bus(K)
    : _, scale_tangents(K)
with {
    error_derivative = ro.cross(2) : - : *(2);  // 2*(pred - truth)
    scale_tangents(K) = route(K+1, 2*K,
        par(i, K, (1, 2*i+1), (i+2, 2*i+2))
    ) : par(i, K, *);
};
```

### 2.3 Optimizers

Operate on gradient signals and produce parameter update deltas.

```faust
// Stochastic Gradient Descent
// gradient → lr * gradient
sgd(lr) = *(lr);

// SGD with momentum
// Maintains exponential moving average of gradients
sgd_momentum(lr, beta) = _ : *(1-beta) : +~(*(beta)) : *(lr);

// RMSProp
// Normalizes gradient by running RMS
rmsprop(lr, rho, epsilon) = _ <:
    _,                                    // gradient
    (^(2) : *(1-rho) : +~(*(rho)))      // running mean of squared gradients
    : _, (sqrt : +(epsilon))
    : / : *(lr);

// Adam optimizer
// Combines momentum (beta1) with adaptive scaling (beta2)
adam(lr, beta1, beta2, epsilon) = _ <:
    (_ : *(1-beta1) : +~(*(beta1))),              // m: first moment estimate
    (^(2) : *(1-beta2) : +~(*(beta2)))             // v: second moment estimate
    // Note: bias correction omitted for simplicity (converges anyway)
    : _, (sqrt : +(epsilon))
    : / : *(lr);
```

### 2.4 Host-Driven Training API

The simplest training pattern: `fad()` produces outputs, the library computes loss and gradients, the host reads gradients via output channels and updates sliders.

```faust
// Example: learn a gain parameter
//
// g = hslider("g", 0.1, 0, 1, 0.001);
// target_gain = 0.7;
// error_expr = _ <: (*(g) - *(target_gain)) : ^(2);
// process = ddsp.train_host(fad(error_expr), 1, ddsp.sgd(0.001));
//
// Outputs: [loss, scaled_grad_1, ..., scaled_grad_K]
// Host reads output 1..K, updates corresponding sliders

train_host(fad_expr, windowSize, optimizer) =
    fad_expr                              // [primal, tangent_1, ..., tangent_K]
    // TODO: need ground truth input for loss — this is a routing problem
    // Simpler: just output tangents scaled by optimizer
    // The host computes loss externally
;
```

**Practical pattern for host-driven training:**

```faust
import("ddsp.lib");

g = hslider("g", 0.1, 0, 1, 0.001);
target = 0.7;

// Error expression — scalar, differentiable w.r.t. g
error = _ <: (*(g) - *(target)) : ^(2);

// fad(error) → [error_value, d_error/d_g]
// Host reads both outputs:
//   output 0 = error (monitor convergence)
//   output 1 = gradient (use to update g)
process = fad(error);
```

The host loop:
```python
for each buffer:
    outputs = dsp.compute(input)
    loss = mean(outputs[0])
    for i, ctrl in enumerate(differentiable_controls):
        grad = mean(outputs[1 + i])
        ctrl.value -= learning_rate * grad
        ctrl.value = clamp(ctrl.value, ctrl.min, ctrl.max)
```

### 2.5 Semi-Automated Training (Faust-side optimizer)

For cases where the user wants the optimizer logic in Faust (e.g. Adam momentum state), but the host still drives the parameter update:

```faust
// Outputs: [loss, adam_update_1, ..., adam_update_K]
// The adam state (momentum, variance) is maintained in Faust via recursion
// The host applies the updates to sliders

process = fad(error)
    : _, par(i, K, adam(0.001, 0.9, 0.999));
```

### 2.6 Example Programs

| File | Description | Controls | Signals |
|------|-------------|----------|---------|
| `examples/ddsp_gain_learn.dsp` | Learn gain to match target | g | 2 (error + grad) |
| `examples/ddsp_gain_bias_learn.dsp` | Learn gain + DC bias | g, b | 3 (error + 2 grads) |
| `examples/ddsp_filter_learn.dsp` | Learn lowpass cutoff | fc | 2 |
| `examples/ddsp_oscillator_learn.dsp` | Learn oscillator frequency | freq | 2 |

### 2.7 Testing

- Each example compiles through the signal pipeline
- Host-driven gradient descent test: programmatic loop that verifies convergence
  (compile DSP, run N iterations updating sliders, assert parameter converges)

---

## Phase 3 — Self-Contained Training in Faust

### Goal

Enable fully self-contained gradient descent within a single Faust program, without host intervention. Parameters are recursive state variables updated by FAD gradients at each sample.

### 3.1 The Core Challenge

With `fad()`, tangent outputs are **extra signal outputs**. They cannot be fed back into the computation that produced them. The faust-ddsp project solves this by never using `fad()` — instead, dual signals flow explicitly through the graph, making gradient feedback trivial via `~`.

To achieve self-contained training with `fad()`, we need a way to:
1. Maintain learnable parameters as recursive state (not UI controls)
2. Differentiate the error computation w.r.t. those parameters
3. Route the gradient back to update the parameters

### 3.2 Approach A — Perturbation Variable Trick

Use `fad()` with a dummy perturbation variable `eps` (always 0). The actual parameter is recursive state. The chain rule gives `dE/deps = dE/dg` at `eps=0`.

```faust
eps = hslider("eps", 0, -1, 1, 0.001);
lr = 0.001;
target = 0.7;

// g is recursive state, updated each sample
// error = (x * (g + eps) - x * target)^2
// d_error/d_eps|_{eps=0} = d_error/d_g

// The FAD tangent for eps IS the gradient for g
// Route it back via ~ to update g

learnable_gain(g_prev, x) = g_new, y
with {
    g = g_prev + eps;
    y = x * g;
    ref = x * target;
    err = (y - ref)^2;
    // Need gradient here — but it's a FAD output, not available inline
};
```

**Problem:** The gradient is an extra output of `fad()`, produced *after* the Rec group is built. Inside the Rec body, you can't access it because it doesn't exist yet.

### 3.3 Approach B — Compiler Extension: `fadGrad(expr, control_index)`

Add a new primitive that extracts a specific tangent signal *inline*, making it available for feedback routing.

```faust
// fadGrad(expr, i) returns the i-th tangent of fad(expr) as a signal
// This tangent is available for routing within the same graph

eps = hslider("eps", 0, -1, 1, 0.001);
lr = 0.001;

// Error expression
error(g, x) = (x * (g + eps) - x * target)^2;

// Extract gradient inline
grad = fadGrad(error(g, x), 0);  // d_error/d_eps

// Update g via recursion
g = 0.5 - lr * grad : +~_;
```

**Implementation:** `fadGrad(expr, i)` would:
1. Run `fad()` transformation on expr
2. Extract tangent signal at index `i`
3. Return it as a regular signal (not an extra output)

This is a **compiler-level change** in `propagate_inner` for a new `FlatNodeKind::FADGrad`.

### 3.4 Approach C — Two-Pass Architecture (inspired by faust-ddsp)

Use the faust-ddsp pattern: explicit dual signals with `~` feedback, but leverage `fad()` to generate the duals automatically in the first pass.

```faust
// Step 1: fad(expr) produces [primal, grad_1, ..., grad_K]
// Step 2: Route grads into a feedback loop that updates parameters
// Step 3: Parameters come back as inputs to the next sample

// The key insight from faust-ddsp:
// diffVar(nvars, I, graph) = -~_ <: attach(graph), par(i,nvars,i+1==I);
//
// This creates a leaky integrator: weight -= gradient each sample

// For faust-rs, the equivalent pattern:
// process = (fad(error_with_eps) : route_grads_to_feedback) ~ (gradient_to_weight_updates)
```

**Concrete implementation:**

```faust
// Self-contained gain learning
// State: [g] fed back via ~
// Outputs: [y] (the processed audio)

lr = 0.001;
target = 0.7;
eps = hslider("eps", 0, -1, 1, 0.001);

// Forward pass: g_prev comes from feedback, x is audio input
forward(g_prev, x) = y, error, g_prev
with {
    g = g_prev + eps;
    y = x * g;
    ref = x * target;
    error = (y - ref)^2;
};

// fad(forward) differentiates w.r.t. eps
// Outputs: [y, error, g_prev, dy/deps, derror/deps, dg_prev/deps]
// The gradient we want is derror/deps (index 4, 0-based)

// Update: g_new = g_prev - lr * gradient
// Route g_new back as feedback

process = (
    fad(forward)
    : route(6, 2,
        (1, 1),           // y → output
        (5, 2)            // derror/deps → feedback as gradient
    )
    : _, *(lr) : _, neg   // y, -lr*grad
) ~ (+ : max(0) : min(1))  // g += (-lr*grad), clamped
    : _, !;                  // output only y
```

**Challenge:** This requires `fad()` to correctly handle the multi-output `forward` function inside the `~` recursion. With `suppress_fad` in Rec, the FAD expansion happens after the Rec group is built, which means the tangent outputs are available for routing.

### 3.5 Approach D — `diffVar` Primitive (recommended)

Add a new compiler primitive `diffVar(init, lo, hi)` that creates a **learnable parameter** — a recursive state variable that behaves like an hslider for FAD differentiation but is updated internally by gradient feedback.

```faust
// diffVar behaves like hslider for FAD (seed = 1 when differentiating w.r.t. it)
// but its value is recursive state, not host-controlled
g = diffVar(0.5, 0, 1);

target = 0.7;
error = _ <: (*(g) - *(target)) : ^(2);

// fadTrain wraps fad() with an optimizer and feedback loop
process = fadTrain(error, sgd(0.001));
```

**Implementation:**
- `diffVar(init, lo, hi)` → new box primitive, evaluates to a special signal node
- `fadTrain(expr, optimizer)` → applies `fad()`, then routes tangents through the optimizer and back to `diffVar` nodes via recursion
- The compiler generates the complete feedback graph automatically

**This is the most user-friendly approach** but requires significant compiler work:
1. New box type: `BoxDiffVar`
2. New signal type: `SigDiffVar` (with init, bounds)
3. `fadTrain` propagation: detects `diffVar` nodes, creates Rec group with gradient feedback
4. Optimizer as a signal transform applied to gradients before feedback

### 3.6 Implementation Priority

| Approach | Complexity | User Experience | Recommended |
|----------|-----------|-----------------|-------------|
| A (perturbation) | Low | Poor (manual eps trick) | No |
| B (`fadGrad`) | Medium | Medium (new primitive) | Maybe later |
| C (two-pass) | Medium | Medium (explicit routing) | Yes, for power users |
| D (`diffVar`) | High | Excellent (fully automatic) | Yes, as end goal |

**Recommended order:**
1. **Phase 2** (this document, §2): Host-driven training with `ddsp.lib` — no compiler changes needed
2. **Phase 3a**: Approach C — document the two-pass pattern, provide examples
3. **Phase 3b**: Approach D — `diffVar` + `fadTrain` compiler primitives for seamless self-contained training

---

## File Summary

| File | Action | Phase |
|------|--------|-------|
| `lib/ddsp.lib` | **New** — loss functions, optimizers, gradient scalers | 2 |
| `examples/ddsp_gain_learn.dsp` | **New** — host-driven gain learning | 2 |
| `examples/ddsp_filter_learn.dsp` | **New** — host-driven filter learning | 2 |
| `examples/ddsp_oscillator_learn.dsp` | **New** — host-driven oscillator learning | 2 |
| `examples/ddsp_selfcontained_gain.dsp` | **New** — self-contained gain learning (approach C) | 3a |
| `crates/boxes/src/tags.rs` | Add `BOX_DIFF_VAR_TAG` | 3b |
| `crates/boxes/src/matcher.rs` | Add `DiffVar` variant | 3b |
| `crates/propagate/src/lib.rs` | Add `FlatNodeKind::DiffVar`, `FlatNodeKind::FADTrain` | 3b |
| `crates/propagate/src/forward_ad.rs` | Handle `SigDiffVar` in differentiation | 3b |
