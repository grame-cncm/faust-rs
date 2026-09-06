# Using `rad(expr, seeds)` for Gradient Descent

Verified against the implementation and runtime examples on 2026-07-22.

This note shows how to use the reverse-mode AD primitive
`rad(expr, seeds)` from a host program. It complements
[`rad-note-en.md`](rad-note-en.md), which describes the algorithm
and rule table.

`rad` is a `faust-rs` extension; the C++ Faust reference compiler used by this
project does not currently recognize it.

## What you get from RAD today

RAD compiles `rad(expr, seeds)` into the output bundle

```text
[primals…, ∂ sum(primals) / ∂ s_0, …, ∂ sum(primals) / ∂ s_{N-1}]
```

with an implicit all-ones cotangent on every primal output. For
temporal or recursive bodies, the gradient lanes are per-sample
contribution signals for the current `compute(count)` block. Sum them
over the block in the host, or inside Faust when a DSP-level reduction
is needed and the block size is known through `ma.BS`.

The supported symbolic rule set largely matches FAD for feed-forward code,
while temporal/recursive bodies route through the `BlockReverseAD` fallback:
arithmetic, trig, transcendentals, `pow`, `atan2`, `min`/`max`,
`select2`, casts, read-only tables, unary FFun, pass-through wrappers,
delay/prefix forms, and recursive feedback. Mutable tables,
soundfiles, non-unary or unrecognised foreign functions still surface
a structured `RadUnsupportedNode` diagnostic — never a silently wrong
gradient.

In short: feed-forward DSPs built from the listed rule families are fittable by
gradient descent when their parameters are explicit seeds. Temporal/recursive
DSPs are fittable within the current block-local BRA horizon when their lowered
signal families are covered by the BRA backward rules.

## Pattern: host-driven gradient descent

The reference implementation is at
[`crates/compiler/examples/rad_gradient_descent.rs`](../crates/compiler/examples/rad_gradient_descent.rs).
Run it with:

```bash
cargo run -p compiler --example rad_gradient_descent
```

The loop is structured as follows.

### 1. Author the differentiated DSP

Expose the parameters as UI controls and wrap the loss-or-output
expression in `rad(...)`:

```faust
gain = hslider("gain", 1.0, -4.0, 4.0, 0.001);
bias = hslider("bias", 0.0, -4.0, 4.0, 0.001);
process = rad(gain * _ + bias, (gain, bias));
```

This is `tests/corpus/rad_gain_bias_train.dsp`. The compiled program
has 1 audio input and 3 outputs:
`[primal, ∂primal/∂gain, ∂primal/∂bias]`.

### 2. Compile and instantiate via the compiler facade

```rust
use compiler::{Compiler, SignalFirLane};
use codegen::backends::interp::{FbcDspInstance, InterpOptions, read_fbc};

let compiler = Compiler::new();
let fbc = compiler
    .compile_file_default_to_interp_with_lane(
        &path,
        &InterpOptions::default(),
        SignalFirLane::TransformFastLane,
    )?;
let mut reader = std::io::Cursor::new(fbc);
let mut factory = read_fbc::<f32>(&mut reader)?;
let mut instance = FbcDspInstance::new(&mut factory);
instance.init(48_000);
```

### 3. Discover slider heap offsets

The interp backend exposes the UI-instruction list and the per-slot
heap accessors needed to read and write slider state from the host:

```rust
use codegen::backends::interp::opcode::FbcOpcode;

let ui = instance.ui_instructions().to_vec();
let gain_offset = ui
    .iter()
    .find(|i| matches!(i.opcode, FbcOpcode::AddHorizontalSlider) && i.label == "gain")
    .expect("gain slider")
    .offset;
let bias_offset = ui
    .iter()
    .find(|i| matches!(i.opcode, FbcOpcode::AddHorizontalSlider) && i.label == "bias")
    .expect("bias slider")
    .offset;
```

### 4. Iterate

Each iteration generates a batch of inputs and a synthetic target,
runs `try_compute`, reads the gradient lanes from the output bundle,
applies the loss-specific chain rule (`∇loss = Σ 2·err·∂out/∂p` for
mean-squared error), and writes the updated parameters back through
`set_real_zone`:

```rust
let inputs: [&[f32]; 1] = [&x];
let mut out_primal = vec![0.0_f32; BLOCK_LEN];
let mut out_dgain  = vec![0.0_f32; BLOCK_LEN];
let mut out_dbias  = vec![0.0_f32; BLOCK_LEN];
let mut outs: [&mut [f32]; 3] = [&mut out_primal, &mut out_dgain, &mut out_dbias];
instance.try_compute(BLOCK_LEN as i32, &inputs, &mut outs)?;

let mut grad_gain = 0.0_f32;
let mut grad_bias = 0.0_f32;
for k in 0..BLOCK_LEN {
    let err = out_primal[k] - target[k];
    grad_gain += 2.0 * err * out_dgain[k];
    grad_bias += 2.0 * err * out_dbias[k];
}
gain -= LEARNING_RATE * grad_gain / BLOCK_LEN as f32;
bias -= LEARNING_RATE * grad_bias / BLOCK_LEN as f32;
instance.set_real_zone(gain_offset, gain);
instance.set_real_zone(bias_offset, bias);
```

The reference implementation recovers the true `gain` and `bias`
within `~1e-6` in 400 iterations of 512-sample batches.

## Pattern: block-local recursion

Recursive DSPs route through `BlockReverseAD` today. The runtime runs
the primal recursion forward over the current `compute(count)` block,
records the needed primal intermediates in BRA tapes, then runs the
adjoint sweep backward over the same block with terminal-zero adjoint
state at the end of each `compute()` call.

The tapes hold `-bra-tape N` samples (default 8192, a power of two): a
block longer than the tape wraps its index and the gradients of the
tail are wrong, so a host that differentiates a loss over a whole
impulse response in one `compute` call sizes the tape to it
(`-bra-tape 32768` for a two-second response at 16 kHz; `--bra-tape` in
`faustprobe`). The forward tapes then cost `N` samples per taped signal,
which is why the default stays small.

One-pole example:

```faust
p = 0.5;
process = rad((_ : + ~ *(p)), p);
```

With input `x[n]`, the primal is `y[n] = x[n] + p*y[n-1]`. The output
bundle is `[y, dp]`, where `dp[n] = lambda[n] * y[n-1]` and
`lambda[n] = 1 + p*lambda[n+1]` for the implicit all-ones objective.
`dp` is a contribution lane, not an already reduced scalar.

A two-state coupled form can be written with standard library routing:

```faust
import("stdfaust.lib");
p = 0.5;
q = 0.25;
core = (ro.interleave(2, 2) : (+, +)) ~ ((*(p), *(q)) : ro.cross(2));
process = rad((_, _) : core, (p, q));
```

The output bundle is `[y0, y1, dp, dq]`. The host usually accumulates
`sum(dp)` and `sum(dq)` over the block before applying an optimizer
step. If the reduction must happen in DSP code, use `ma.BS` to make
the block length explicit and reduce the contribution lanes with a
block-aware construction.

The same block-local rule is used for LTI, LTV, and nonlinear
recursive classes. The classifier still records whether a recursive
body is `LinearTranspose`, `BlockLinearTimeVarying`, or `BpttRequired`,
but public RAD dispatch currently uses BRA for all of them while the
specialized recursive paths remain dormant.

## Pattern: delay lines and FIR taps

Non-recursive delay and prefix nodes also use the BRA fallback. For FIR
taps, it can still be practical to lift the delay line out of the
differentiated body and feed the delayed taps as separate audio
channels when the host already owns that buffer:

```faust
c0 = hslider("c0", 0.25, -2.0, 2.0, 0.001);
c1 = hslider("c1", 0.25, -2.0, 2.0, 0.001);
c2 = hslider("c2", 0.25, -2.0, 2.0, 0.001);
c3 = hslider("c3", 0.25, -2.0, 2.0, 0.001);

kernel(x0, x1, x2, x3) = c0*x0 + c1*x1 + c2*x2 + c3*x3;
process = rad(kernel, (c0, c1, c2, c3));
```

The host buffers `x[n-k]` and writes `[x_n, x_{n-1}, x_{n-2}, x_{n-3}]`
to the four input channels each frame. See
[`tests/corpus/rad_fir_taps_external_delays.dsp`](../tests/corpus/rad_fir_taps_external_delays.dsp).

## Pattern: LMS adaptive filtering (host-managed delay line)

A 3-tap FIR notch with a single tunable angular frequency:

```faust
omega = hslider("omega", 1.0, 0.01, 3.0, 0.0001);
notch(xn, xn1, xn2) = xn - 2.0 * cos(omega) * xn1 + xn2;
process = rad(notch, omega);
```

is `tests/corpus/rad_adaptive_notch_omega.dsp`. The transfer function
is `H(z) = 1 - 2·cos(ω)·z⁻¹ + z⁻²`, which places zeros on the unit
circle at `e^(±j·ω)`. Minimising the output power
`J(ω) = E[y²]` drives `ω` to the strongest input frequency — the
classical LMS adaptive notch.

[`crates/compiler/examples/rad_adaptive_notch.rs`](../crates/compiler/examples/rad_adaptive_notch.rs)
ships the full host loop:

- synthesises a noisy sinusoid at `OMEGA_TARGET = 1.3 rad/sample`,
- buffers `x[n], x[n-1], x[n-2]` over a 512-sample moving window,
- runs the rad-compiled DSP block-by-block,
- accumulates `loss = mean(y²)` and `grad = mean(2·y·∂y/∂ω)`,
- updates `ω` with plain SGD, projects back into the slider's
  declared range, and writes through `set_real_zone`.

Starting from `ω = 0.4` it converges to within `2e-4` of the target
in under 50 iterations. The remaining loss is the additive noise
floor `σ²`, which is exactly what an LMS adaptive notch should leave
behind.

Run with:

```bash
cargo run --release -p compiler --example rad_adaptive_notch
```

This is the recognisable adaptive-filtering use case for RAD: any FIR
or memoryless nonlinear filter parameterised by one or more sliders is
fittable today, and host buffering remains useful when the application
already owns the delay line.

## Pattern: stateless waveshapers

A polynomial or `tanh`-based waveshaper without feedback fits
directly. See
[`tests/corpus/rad_waveshaper_polynomial_coefs.dsp`](../tests/corpus/rad_waveshaper_polynomial_coefs.dsp)
and
[`tests/corpus/rad_static_softclip_drive.dsp`](../tests/corpus/rad_static_softclip_drive.dsp).
For the polynomial case, the host pre-computes `(x, x², x³)` and
feeds them as three audio channels to keep `_` from getting expanded
into independent inputs by Faust's wire substitution.

## Performance note

The benchmark
[`crates/compiler/examples/rad_vs_fad_perf.rs`](../crates/compiler/examples/rad_vs_fad_perf.rs)
compares RAD vs FAD on representative feed-forward and recursive
shapes. On the current
pipeline (release builds, signal-prepare + CSE active) the bytecode
size and per-frame compute time are close between the two modes on
the feed-forward shapes, including the deep multiplicative chain that
plan §17 flagged as the canonical adjoint-sum-growth stress case. The
same harness now also includes recursive one-pole and coupled
state-space cases.

In other words: at the scales explored here, the simplification
pipeline already absorbs the symbolic-growth cost of reverse-mode
adjoint accumulation. The recursive cases should be read as
implementation guardrails rather than statistically rigorous
benchmarks.

Run with:

```bash
cargo run --release -p compiler --example rad_vs_fad_perf
```

## Limits to keep in mind

- **Block-local temporal boundary.** `delay`, `prefix`, and recursive
  bodies use the current `compute(count)` block as the reverse horizon
  through `BlockReverseAD`. This is exact for the block-local objective,
  with zero terminal adjoint state, not a cross-call infinite-horizon adjoint.
- **One-sample horizon inside the graph.** A gradient consumed by a
  forward-time expression -- `p_next = p - lr * (rad(loss(p), p) : !, _)`
  in an adaptation recursion -- is produced at the sample that consumes it,
  so the sweep sees that sample only: through a recursion it returns the
  direct term, the past state held fixed (the pseudo-linear-regression
  gradient of adaptive IIR filtering), where `fad` carries the derivative
  through the recursion; for a feed-forward body both are the same. The
  `_rad` bus loops of `libraries/optimizers.lib` are built on this.
- **Implicit all-ones cotangent.** Multi-output `expr` produces the
  gradient of `sum(primals)`. A future `vjp(expr, cotangent, seeds)`
  primitive will expose custom output cotangents.
- **No automatic seed discovery.** Seeds must be listed explicitly,
  same as for FAD. UI-control annotations are not consulted, and seed
  recognition is Signal IR identity rather than algebraic equivalence.
- **Clock domains.** RAD across `ondemand`, `upsampling`, or `downsampling`
  clock-domain machinery is rejected with a structured diagnostic; a
  clock-aware reverse tape is not implemented yet. A `rad` whose expression
  and seeds live inside one `ondemand` body is supported: the block's inputs
  and the foreign constants are leaves of the sweep, and the clocked wrapper
  is passed through (`libraries/ddsp-examples-en.md`, example 11).
- **Hard unsupported families.** Mutable tables, soundfiles, and unknown or
  non-unary foreign functions are rejected rather than assigned a fabricated
  gradient.
- **Table-index approximation.** Read-only table lookup differentiates the
  index using a symmetric finite-difference slope; table contents remain
  constant.
- **Non-smooth points.** Rules are not automatically regularized; for example,
  the current `abs` derivative uses `x / abs(x)` and may produce `NaN` at zero.

## Source pointers

- [`crates/propagate/src/reverse_ad.rs`](../crates/propagate/src/reverse_ad.rs)
  — RAD propagation pass.
- [`crates/codegen/src/backends/interp/instance.rs`](../crates/codegen/src/backends/interp/instance.rs)
  — `ui_instructions`, `get_real_zone`, `set_real_zone`.
- [`crates/compiler/examples/rad_gradient_descent.rs`](../crates/compiler/examples/rad_gradient_descent.rs)
  — reference SGD loop.
- [`crates/compiler/examples/rad_adaptive_notch.rs`](../crates/compiler/examples/rad_adaptive_notch.rs)
  — adaptive 3-tap notch filter, LMS convergence on output power.
- [`crates/compiler/examples/rad_vs_fad_perf.rs`](../crates/compiler/examples/rad_vs_fad_perf.rs)
  — RAD-vs-FAD comparison harness.
- [`rad-note-en.md`](rad-note-en.md) — RAD algorithm and rule
  table.
- [`porting/reverse-ad-rad-implementation-plan-2026-04-27-en.md`](../porting/reverse-ad-rad-implementation-plan-2026-04-27-en.md)
  — full implementation plan including the stateful-RAD §19 analysis.
