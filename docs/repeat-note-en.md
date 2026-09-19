---
title: "Note: the repeat primitive, and the clocked wrappers built on it"
author: "Stéphane Letz and Claude Fable 5.1"
date: "2026-09-19"
---

# `repeat`: a bounded loop with an exit, and `ondemand` / `upsampling` / `downsampling` as its derived forms

French version: [repeat-note-fr.md](repeat-note-fr.md) (same content; keep both in sync on amendment).

This note states the semantics of `repeat`, a clocked wrapper proposed on 2026-09-18, and shows how the three clock-domain primitives of faust-rs, `ondemand`, `upsampling` and `downsampling`, are derived forms of it. It is written from the Faust programmer's point of view and says nothing about how the compiler implements any of it; that is the subject of `porting/repeat-counted-loop-with-exit-analysis-and-plan-2026-09-18-en.md`. The three primitives themselves are presented in [ondemand-note-en.md](ondemand-note-en.md), which this note assumes.

Every number quoted was measured on 2026-09-18 with `faustprobe`, double precision, 48 kHz, on the library form of `repeat` written with the existing wrappers (the plan, section 1.5); the semantics below is therefore not a design on paper but a behaviour that runs.

## 1. Why `repeat`

### 1.1 The loops Faust has

Faust has three ways to repeat a computation, and none of them runs a loop whose end is decided by the loop:

- `par`, `seq`, `sum` and `prod` unroll at compile time. The count is a literal, the graph grows with it, and nothing is decided at run time.
- `~` runs one step per sample. A recursion is a loop whose iterations are the samples; it cannot take two steps in one sample.
- an `ondemand` block with an integer clock runs its body `H` times per sample, `H` a signal: a loop with state at run time, the first Faust has had. But `H` is a signal of the outer domain, computed before the block runs. The count may depend on the inputs, on the held state, on the previous solution; it cannot depend on what the iterations compute.

### 1.2 The loops that end on a condition

A whole class of numerical methods is defined by an *until*:

- root finding and fixed-point iteration to a tolerance: Newton, Halley, the secant; the implicit equation of a virtual-analog stage (a diode clipper, a zero-delay-feedback ladder, a feedback saturator), the per-sample nonlinear solve of a wave-digital or nodal circuit model;
- iterative refinement of a small linear system per sample (Jacobi, Gauss–Seidel, conjugate gradient on a nodal matrix), stopped on the residual;
- adaptive step control: an integrator that retries a step with a smaller size until its error estimate is below the tolerance, the number of sub-steps known only from the attempts;
- a backtracking line search in an optimizer: halve the step until the loss falls;
- a search that stops at the first acceptable candidate: a grid, a bisection on a parameter, a restart from a perturbed point;
- the two evaluations of a stochastic step (SPSA), or a restart, in one sample rather than over two frames.

Their common shape: a body with state, a bound that real time needs anyway, and an exit read on the iterate. The bound is the budget; the exit is what makes the budget a maximum rather than a cost.

### 1.3 What writing them without `repeat` costs

They can be written today, and the Newton solver of §6.1 was, in two layers: an outer integer `ondemand` whose clock is the budget, decided by the residual of the warm start, and inside it a boolean `ondemand` gating each step on the residual of the held iterate. It runs, it is exact, and it is what `repeat` is made of. What it costs:

- the unused iterations of the budget each evaluate a residual, an `or` and an `if`: 4 to 12 % of the solver on a 100 Hz sine, measured;
- the loop is spread over two blocks, a feedback and a held flag. A reader sees two guards, not a loop, and a library cannot document "this block stops when it has converged" as the contract of one construct;
- the shape has to be recognised, by people and by tools, each time it is written, and a small change (the flag fed back through the wrong lane, a tangent on the flag) silently turns it into something else.

### 1.4 Why a name in the language

`repeat` names that shape: the bound as the clock, the exit as the last output of the body, the first iteration unconditional, and the time model of the three wrappers. Beyond the library form, which already gives the semantics, the name buys four things, none of them about the generated code:

- **a contract.** "This block runs at most `H` times and stops when its flag falls" is said in one construct, readable by a person and known to the compiler;
- **the bound and the exit as rules.** The worst-case cost of a sample stays `H × body`, for all four forms alike; the evaluation point of the flag, after the outputs of its iteration, is a rule of the language and not a convention on a feedback;
- **one rule for `fad`.** The flag carries a primal only; a tangent cannot land where the exit is read (§7);
- **one loop, three specialisations.** The three existing wrappers become its derived forms (§4), which is the conceptual gain of this note: the two readings of `ondemand` disappear, `downsampling` is a clock expression, `upsampling` an input rewrite, and the rate annotation is the one thing left that is not the loop.

## 2. `repeat` on one page

```faust
repeat(C)
```

**Arity.** If `C : u → v+1` then `repeat(C) : u+1 → v`. As for the three other wrappers, the extra **first input** is the clock `H`. The **last output** of `C` is a *continue flag*, consumed by the wrapper and not exposed. `C` must have at least one output besides the flag.

**Per outer sample**, with `n = int(H)` (the clock truncated to an integer, as the three wrappers do; a negative clock counts as 0):

```tsv
clock	effect
n = 0	the body does not run; the outputs hold their last value (0 at start)
n = 1	the body runs once; its flag is read but cannot matter
n ≥ 2	the body runs, then again while its flag was non-zero, at most n times
```

The clock is a **bound**, never a promise: the body runs between 1 and `n` times when `n > 0`, and the first iteration is unconditional. The exit is decided **inside**, on what the iteration just computed. In C-like notation, per outer sample:

```c
if (n > 0) do { body; } while (flag && ++k < n);
```

A `while` without a bound is not offered, on purpose: in real time a block whose iteration does not converge is an audio thread that never returns. `repeat` keeps the worst-case cost of a sample bounded by the clock's range, which the type system knows.

## 3. The semantics, precisely

### 3.1 Two times

The program outside the block advances in *outer time* `t = 0, 1, 2, …`, one step per audio sample. The body `C` advances in *local time* `τ = 0, 1, 2, …`, one step per **executed iteration**, across all outer samples. Every stateful construct in the body (a delay, a `~` recursion, a table, `ba.time`, an oscillator) advances once per executed iteration and never otherwise. A definition referenced in the body is instantiated in the body's domain, as for the three wrappers ([ondemand-note-en.md](ondemand-note-en.md), section 3): the same `x @ 1` written inside and outside the block are two delay lines.

### 3.2 The equations

Let `H` be the clock and `x = (x₁ … x_u)` the inputs, both outer signals. Let `C` be the body as a signal processor in local time: from an input stream `X(τ)` it produces `v` output streams `Y(τ)` and a flag stream `F(τ)`, with all its state advancing in `τ`.

Per outer sample `t`:

- `n_t = max(0, int(H(t)))`, the bound;
- `τ_t` is the number of iterations executed before sample `t` (`τ_0 = 0`);
- the inputs are **snapshotted**: `X(τ) = x(t)` for every iteration `τ` executed at sample `t`, constant across the iterations of that sample;
- the number of iterations executed at `t` is `k_t = 0` if `n_t = 0`, otherwise `k_t = min(n_t, 1 + min{ j ≥ 0 : F(τ_t + j) = 0 })` (the first iteration whose flag is 0 is the last one executed; if no flag falls, all `n_t` run);
- `τ_{t+1} = τ_t + k_t`;
- the outputs of the block are those of the **last executed iteration**, held otherwise: `y(t) = Y(τ_{t+1} − 1)` if `k_t > 0`, else `y(t) = y(t−1)`, with `y(−1) = 0`.

This is well founded: `F(τ_t + j)` depends on the state of `C` at `τ_t` and on the snapshot `x(t)`, both known before the iteration runs.

### 3.3 The flag

The flag is evaluated **after** the outputs of its iteration. So the iteration that says "stop" is included, and the held outputs are the ones it produced. Its polarity is *continue*: non-zero means "go on", which reads like `while (flag)`. (The alternative, a *stop* flag, which the idiom `repeat … until` would suggest, is the one decision still open; nothing else in this note depends on it.)

The flag is read only when there is something to continue to: with `n_t = 1` it is computed and ignored, and the type system may not compute it at all when the clock's range lies within [0, 1].

### 3.4 The clock

- **Truncation.** A real clock is cast to an integer before anything else, as for the three wrappers and as the C++ reference does: `0.5` never runs the body, `2.5` runs it at most twice. There is no fractional execution.
- **Constant clocks.** `H ≡ 0`: the body is never instantiated and the outputs are 0. `H ≡ 1`: the body is `C` run once per sample, its flag output dropped; local time is outer time.
- **Boolean range.** When the type system knows `H ∈ [0, 1]`, at most one iteration runs per sample and the flag cannot matter: `repeat` and `ondemand` coincide.
- **`ma.SR` inside is unchanged.** The body sees the outer sample rate, as under `ondemand`: the number of iterations per sample is a signal, so there is no constant ratio to fold into a rate (§4.4).

### 3.5 Nesting

Any wrapper may appear inside any other. The clock of an inner block is a signal of the enclosing body's domain, evaluated once per enclosing iteration. An exit leaves its own loop only.

## 4. The three wrappers as derived forms

The construction has one loop, one time model, one hold rule and one input rule. Each of the three primitives is `repeat` with a constant flag, plus at most one of three orthogonal additions: a clock expression, an input rewrite, or a rate annotation.

### 4.1 `ondemand`

```faust
ondemand(C) ≡ repeat((C, 1))
```

`(C, 1)` is `C` with a constant-1 flag appended in parallel. The body runs `int(H)` times per sample, whatever the range of the clock. The two rows of the `ondemand` table in [ondemand-note-en.md](ondemand-note-en.md) section 2 (a *condition* when the range is within [0, 1], a *count* otherwise) are therefore **one rule**, `int(H) ∈ {0, 1}` being the case where a count is an `if`. The condition reading is an emission of the count, not a second semantics. `ondemand` has no exit: the bound is the count.

### 4.2 `downsampling`

```faust
// the divided clock: a counter in the outer domain, fires when it is 0
fire(H) = (c == 0) with { c = ((+(1) ~ _) - 1) % H; };
downsampling(C) ≡ (fire(H), si.bus(u)) : ondemand(C)      // plus ma.SR = SR / H inside
```

The body runs on the ticks `t = 0, H, 2H, …`, once each (the flag is 1 and the clock is boolean), its inputs sampled at those ticks, its outputs held between them. The counter lives in the outer domain; with a constant period this is `t mod H`, measured identical sample for sample to `downsampling(3)`. With a period that changes between ticks the counter is what defines the firing pattern, and that is a property of the derived form to pin, not of `repeat`. What `downsampling` adds that no combination of the loop can express is the **rate annotation**: `ma.SR` inside the body is `SR / H` (§4.4).

### 4.3 `upsampling`

```faust
// zero-stuffing: the sample arrives on the last of the H iterations, zeros before it
stuff(H, i, x) = x * (i == H - 1);                        // i: the iteration index within the sample
upsampling(C) ≡ (H, x) : repeat((stuff(H, i) : C, 1))    // plus ma.SR = SR * H inside
```

The body runs `int(H)` times per sample, the flag is 1, and the inputs are **not** the snapshot repeated but the snapshot **zero-stuffed**: input `j` of iteration `i` is `x_j(t)` when `i = H − 1` and 0 before. The outputs are those of the last iteration, as for every form. So `upsampling(C)` is the textbook chain: zero-stuffing (`↑₀`, the one `interleave.lib` also uses), `C` at rate `SR · H`, and decimation by keeping the last of each group of `H`.

Why the sample arrives on the **last** iteration and not the first: it is what makes `upsampling(_) = _`, sample for sample and with no delay. With the sample first, the kept output would be that of the last iteration, which saw a 0. The same placement is why `upsampling` admits no exit flag: an early exit would leave before the input arrives. Measured: the zero-stuffed integer `ondemand` is identical sample for sample to `upsampling(3)`.

Two consequences for the programmer. A memoryless body (`upsampling(ma.tanh)`) is the identity on the body, the oversampling doing nothing; oversampling only acts through **state** in the body, the interpolation filter that spreads the sample over the `H` slots and the anti-aliasing filter before the decimation, both the programmer's to write (§6.2). And the zero-stuffing divides the level by `H`, which the interpolation filter's gain must restore (`xi * H` in the example of §6.2).

### 4.4 What the derived forms add

```tsv
form	clock given to the loop	input rewrite	flag	ma.SR in the body
repeat(C)	H, the bound	none	the last output of C	SR
ondemand(C)	H, the count	none	1	SR
downsampling(C)	a counter's firing, 0/1	none	1	SR / H
upsampling(C)	H, the count	zero-stuffing, sample last	1	SR · H
```

The rate annotation is the one thing the loop cannot express. A body that reads `ma.SR` means something different under `upsampling` than under `repeat` with the same clock: a filter designed from `ma.SR` inside `upsampling` is tuned for the oversampled rate, inside `repeat` or `ondemand` for the outer rate. It is a property of the **domain**, set by the two rate wrappers because their factor is the clock, and set by nothing else. (`maths.lib` clamps `ma.SR` at 192 kHz, so above `×4` at 48 kHz a filter of `filters.lib` inside `upsampling` is designed for the wrong rate unless asked for `fc * ma.SR / SRraw`, a design depending on `fc / SR` only; see §6.2.)

### 4.5 Identities

The construction is held to these equalities, sample for sample; the first four were measured on 2026-09-18, the others follow from §3 and §4:

- `ondemand(C) = repeat((C, 1))`: a flag always 1 with `H = 5` counts 5 iterations per sample;
- `repeat((C, 0))` runs exactly once per sample when `int(H) > 0`, including when `H` changes from one sample to the next: it is `ondemand(C)` with the boolean clock `int(H) > 0`;
- `downsampling(3)` = `ondemand` with the divided clock of §4.2;
- `upsampling(3)` = zero-stuffed integer `ondemand`, §4.3;
- `H ≡ 1`: `repeat(C)` is `C` with its flag dropped, `ondemand(C)`, `upsampling(C)` and `downsampling(C)` are `C`;
- `H ≡ 0`: every form outputs 0;
- `upsampling(_) = _`, and `upsampling(C) = C` for a memoryless `C`;
- `downsampling(_)` is a sample-and-hold of period `H`;
- `il.interleave(N, _) = @(N − 1)` (boolean `ondemand`, `interleave.lib`);
- nesting multiplies: `upsampling` with `a` around `upsampling` with `b` runs `a · b` iterations at `SR · a · b`; `downsampling` with `a` around `downsampling` with `b` fires every `a · b` samples at `SR / (a · b)`.

## 5. What the construction is worth

**One semantics.** Local time counting executed iterations, inputs snapshotted at the sample, outputs of the last iteration held, a clock truncated to a bound: every wrapper obeys the same four rules, and the differences between them are the table of §4.4. Nothing in the three primitives needs a rule of its own, and the one apparent duality, the two readings of `ondemand`, disappears.

**The exit belongs to the loop.** Before `repeat`, a stopping criterion computed inside the iteration needed two layers: an outer `ondemand` running a budget, an inner boolean `ondemand` gating each step on the held flag of the previous one. `repeat` says the same thing in one layer, the exit read on the iterate itself, and the two-layer form becomes what `repeat` is made of. Measured on the solver of §6.1: the same steps per sample as the two layers and 4 to 12 % cheaper, the skipped iterations no longer recomputing their residual.

**The bound stays.** `repeat` is not a `while`. What it gives up is exactly what real time cannot have; what it keeps is that the clock's interval bounds the cost of a sample statically, for all four forms alike, and that a budget too small is a wrong answer and not a hang. That last point is a warning: an iteration cut by the bound is silent. A solver written with `repeat` should expose its step count, or a "hit the bound" flag, so that a program can see it.

**The three primitives are held to the C++ reference.** Their semantics is not only the equations of §3 and §4 but 39 impulse-test programs (21 `ondemand`, 9 `upsampling`, 9 `downsampling`) compared sample for sample with the C++ Faust branch that defines them, on every backend. `repeat` has no C++ counterpart; its semantics is pinned by the library form and the identities of §4.5.

**Where it is not uniform**, stated so that nobody is surprised:

- `ma.SR` (§4.4): the only place where a wrapper is more than a loop with a clock, and the one that changes what a body means.
- The truncation of a real clock (§3.4): `0.5` is a clock that never fires. The first version of the `ondemand` note said otherwise and was corrected on 2026-09-18.
- `upsampling` admits no exit and `downsampling` has none to admit (§4.3): the flag is meaningful for `repeat` alone, and for `ondemand` only as a constant.
- The first iteration is unconditional (§2). To skip a sample entirely, the decision is taken outside on the held state, by a clock of 0, which is how "do nothing while the input has not moved" is written (§6.1).
- The polarity of the flag (§3.3) is not decided.

## 6. Emblematic uses

### 6.1 An implicit solver with a run-time step count

`newton(N, F, y0)` of `optimizers.lib` unrolls `N` steps in the graph and restarts from `y0` at every sample. With `repeat` the iteration is a recursion in the body, the block's state is the iterate (the next sample starts from the previous solution, a warm start for free), the number of steps is decided by the residual of the iterate, and the code has the size of one step. On the feedback saturator `y = tanh(x − fb · y)`:

```faust
fb = hslider("fb", 0.8, 0, 0.99, 0.01);                 // the feedback, the parameter the gradient of §7 is taken against
tol = 1e-12;                                             // tolerance on the residual
Kmax = 8;                                                // the budget: at most 8 steps per sample
F(x, y) = y - ma.tanh(x - fb * y);                       // residual
step(x, y) = y - (fad(F(x, y), y) : /);                  // one Newton step, y - F / F_y
iter(xi) = (step(xi) ~ _) <: _, (abs(F(xi, _)) > tol);   // (new iterate, continue while not converged)
solve(x) = sel ~ _
with { sel(yp) = ((abs(F(x, yp)) > tol) * Kmax, x) : repeat(iter); };
```

Outside, the residual of the warm start on the new input decides the clock: 0 when the held solution already satisfies the tolerance (the block does not run, the output holds), the budget `Kmax` otherwise. Inside, one step per iteration while the residual of the new iterate is above the tolerance. With `tol` 1e-12 and `Kmax` 8, against `newton(8, …)` from 0 (10.2 ms per second of audio), equal to it to 1.1e-12:

```tsv
input	steps per sample, mean and largest	cost per second of audio
constant	0 after the first sample	0.25 ms
sine, 100 Hz	2.4, 3	5.3 ms
sine, 2 kHz	3.0, 3	6.1 ms
white noise	3.5, 5	7.3 ms
```

A solver that costs nothing while its input does not move, and the steps it needs otherwise. The same shape is a diode clipper, a zero-delay-feedback ladder, any virtual-analog stage with an implicit nonlinearity, and any fixed-point iteration whose convergence is known only from its iterate.

### 6.2 An oversampled nonlinearity, as `upsampling`

A learned waveshaper under a spectral loss needs oversampling, since aliasing is a spectral error the gradient would otherwise learn to cancel. `upsampling` is the oversampled stage in one block: the zero-stuffing by contract, an interpolation filter at the inner rate, the nonlinearity, an anti-aliasing filter, the last iteration as the decimation:

```faust
H = hslider("factor", 4, 1, 16, 1);                     // the oversampling factor, a signal
SRraw = fconstant(int fSamplingFreq, <math.h>);
lp = fi.lowpass(6, 20000 * ma.SR / SRraw);            // designed at the inner rate, see §4.4
stage(x) = (H, x) : upsampling(\(xi).(xi * H : lp : ma.tanh : lp));
```

Measured on a 5250 Hz sine at a drive of 8, as the ratio of the harmonic energy to the rest of the spectrum:

```tsv
factor	harmonic-to-rest	cost per second of audio
plain ma.tanh	13.5 dB	
2	30.7 dB	2.0 ms
4	31.4 dB	3.8 ms
8	30.1 dB	7.3 ms
```

The plateau is the transition band of the sixth-order Butterworth, not the mechanism's. The factor can be a signal (`×4` while an envelope follower is above a knee, `×1` below); the filter inside then becomes time-varying at the switch. The gradient crosses the block: the derivative of the mean squared output with respect to the drive matches the central difference to 2.9e-6 at `×4`.

### 6.3 Several optimizer steps per tick, as `ondemand`

With `H = K · frame_clock(N)`, a whole descent loop runs `K` times on the frame tick and never in between, warm-started from the previous frame: the per-frame analysis by synthesis of differentiable DSP.

```faust
K = 4;                                                   // optimizer steps per frame tick
learn(x) = (K * il.frame_clock(64), x)
         : ondemand(\(xi).(op.descend_1D(\(p).(op.mse(p * xi, 0.7 * xi)), op.sgd_g(0.1), -4, 4, 0, 0)));
```

On a gain from 0 with SGD at 0.1: with `K = 1` the parameter is at 0.692 after 20 frames; with `K = 4` at 0.7 to 1e-8 after 20; with `K = 16` after
5. The price is where the work lands, on one sample; the boolean form spreads it over the frame at the cost of a frame of latency.

### 6.4 A search as a loop, with an exit

A grid of `M` candidates tried one per iteration, the candidate a function of local time, the state keeping the best: code of the size of one evaluation where `multistart_1D` copies the graph `M` times.

```faust
M = 16;                                                  // candidates
clock = il.frame_clock(64);                              // one grid per frame tick
loss(p, x) = op.mse(p * x, 0.37 * x);                    // the target gain is 0.37
body(xi) = best ~ (_, _)
with {
    i = ((+(1)) ~ _) - 1 : %(M);                  // local time: the iteration index
    cand = -1.0 + 2.0 * i / (M - 1);
    best(pb, lb) = select2(better, pb, cand), select2(better, lb, l)
    with { l = loss(cand, xi); better = (l < lb) | (i == 0); };
};
grid = (M * clock, _) : ondemand(body);
```

Sixteen candidates on [−1, 1] for a target of 0.37 elect 0.333 in one tick. What `repeat` adds is the exit. The same grid, stopping at the first candidate whose loss is under a threshold, the clock still bounding it at `M`:

```faust
thr = 1e-3;                                              // acceptable loss
body(xi) = (best ~ (_, _)) : (_, >(thr))                 // (best so far, continue while its loss is above the threshold)
with {
    i = ((+(1)) ~ _) - 1 : %(M);
    cand = -1.0 + 2.0 * i / (M - 1);
    best(pb, lb) = select2(better, pb, cand), select2(better, lb, l)
    with { l = loss(cand, xi); better = (l < lb) | (i == 0); };
};
grid = (M * clock, _) : repeat(body);
```

On a constant input the block elects the same 0.333 and stops as soon as a candidate is under the threshold, where the `ondemand` form always runs the `M` candidates. More generally, the exit: a flag "no candidate under the threshold yet" stops the grid at the first acceptable one; a step halved at every iteration with the flag "the loss has not fallen yet" is a backtracking line search, stopping at run time where today's forms run their whole budget; the two evaluations of an SPSA step fit in one sample instead of two frames.

### 6.5 Where the exit is only known inside

An adaptive integrator (RK45 with step rejection on a stiff circuit) knows its number of sub-steps only from the error estimate of each attempt: a `repeat` whose flag is "the step was rejected", with the bound as the safety. By contrast a rational resampler stays a count: the number of output samples per input sample is read off the phase accumulator before the loop, so it is an integer `ondemand`, `repeat` with a flag of 1.

### 6.6 The boolean forms are unchanged

Event-triggered work (`ondemand` with a 0/1 clock), control-rate computation (`downsampling` with a period), frame-rate spectral processing (`interleave.lib` on a boolean `ondemand`): everything of [ondemand-note-en.md](ondemand-note-en.md) sections 4 and 5 is `repeat` with a constant flag and a boolean clock, and reads exactly as before.

## 7. Differentiation

`fad` crosses `repeat` as it crosses the three wrappers: each held output carries its primal and its tangents, the flag carries its primal only. The clock and the flag are opaque to the derivative, which is right: an iteration count and a comparison are discrete. When the body is a contractive iteration that exits on convergence, the tangent at the exit is the derivative of the fixed point (Christianson, 1994), so the block gives the implicit derivative without a rule of its own. Measured on the solver of §6.1: the tangent of the solution with respect to `fb` on a constant input is −0.240418, the central difference −0.24042. `rad` does not cross a clocked boundary, `repeat` included; learning through these blocks is `fad`.

## 8. Open points, semantic only

Four questions are still to be decided. None of them touches the model of §3; each fixes a convention that every program written with `repeat` will depend on, which is why they are listed here rather than left to the implementation.

- **The polarity of the flag** (§3.3). The last output of the body can mean "continue" (non-zero: run again) or "stop" (non-zero: leave). The model is the same either way; what changes is the sense of one bit, and it changes it in every program: a body whose flag is `abs(F) > tol` under the continue reading must become `abs(F) <= tol` under the stop reading, and a program written for one and compiled under the other runs one iteration where it should run to convergence, or the whole budget where it should stop at once. For *continue*: it is what the library form of §1.5 of the plan implements and measures, and it follows the convention of `op.gated`, whose last output is a gate with 1 meaning "active". For *stop*: the name reads `repeat … until`, and a convergence test is naturally a stop condition ("converged, so stop"). The decision has to be taken before the first program is written, and cannot be revisited afterwards without changing their meaning.

- **The minimal body.** May the body have the flag as its only output? A loop with state and no output has no observable effect in Faust, which has no side effects, so accepting it would admit a block that computes nothing anyone can read. The proposal is to refuse it, as a `gated` block without an output besides its gate is refused. The alternative, to accept it as a block with `u+1` inputs and no output, costs nothing in the model and buys nothing either; the point is only to say which, so that the arity rule `C : u → v+1`, `repeat(C) : u+1 → v` has `v ≥ 1` written into it or not.

- **A period that changes under `downsampling`** (§4.2). The derived form counts in the outer domain, and two counters agree while the period is constant and disagree when it changes. With `t mod H(t)`, the period 3 for `t = 0 … 4` then 2 fires at `t = 0, 3, 6`. With a counter that wraps, `c ← (c + 1) mod H` evaluated at every tick and firing at `c = 0`, the same clock fires at `t = 0, 3, 5`: the counter holds 1 at `t = 4` and wraps at 2 on the next tick. The measurement of 2026-09-18 covered the constant period only. The counter is what the emission implements and what the C++ reference does, so it is the behaviour to write down as the rule; the point is that it is written down, since a program with a period taken from a slider will meet it.

- **Negative clocks.** `int(H)` can be negative when the clock comes from a slider or a subtraction. The rule proposed is that a negative bound counts as 0: the body does not run and the outputs hold, exactly as for `H = 0`. The alternatives, an error when the clock's interval admits negative values, or the absolute value, are both defensible; the first would refuse programs that today run, the second would run a body a programmer did not ask for. Whichever is chosen, it must be a rule and not what the loop header happens to do with a negative bound. What it does today, checked on 2026-09-19 on a clock from a slider with range [−4, 4]: the C++ reference (`8eebea429`, 2.84.3) emits `if (H != 0) { for (od = 0; od < H; od++) … }` and faust-rs `for (lOd = 0; lOd < H; lOd++)`, so on both a negative clock passes the guard, runs the loop zero times and holds the outputs, exactly as `H = 0` does; neither compiler refuses or warns about the interval. The proposed rule is therefore the current behaviour, made explicit.

## See also

- [ondemand-note-en.md](ondemand-note-en.md) — the three primitives, from the programmer's side
- [ondemand-fft-spectral-comparison-en.md](ondemand-fft-spectral-comparison-en.md) — frame-rate processing on a boolean `ondemand`
- [fad-note-en.md](fad-note-en.md) — forward-mode differentiation
- `libraries/optimizers-overview-en.md` section 2.5 — integer clocks and `upsampling` for differentiable DSP, with the programs and measurements quoted here
- `porting/repeat-counted-loop-with-exit-analysis-and-plan-2026-09-18-en.md` — the analysis, the library form, the compiler design and its plan
