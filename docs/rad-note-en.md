# Reverse-Mode AD in `faust-rs`

Verified against the implementation and runtime tests on 2026-07-22.

Synthesis note describing the reverse-mode automatic differentiation
(RAD) pass implemented in `crates/propagate/src/reverse_ad.rs`, with
emphasis on the algorithm, the rule table, the temporal boundary, and
the relationship to the forward-mode pass.

## 1. Surface

`rad(expr, seeds)` is a two-child node mirroring `fad(expr, seeds)`.
It is a `faust-rs` extension; the C++ Faust reference compiler used by this
project does not currently recognize it.

```faust
x = hslider("x", 1, 0, 10, 0.01);
y = hslider("y", 2, 0, 10, 0.01);
loss = sin(x * y);
process = rad(loss, (x, y));
```

Output bundle layout:

```text
rad(expr, (s_0, …, s_{N-1})) =
    [ expr_0, expr_1, …, expr_{M-1},
      ∂ sum(expr_i) / ∂ s_0,
      ∂ sum(expr_i) / ∂ s_1,
      …,
      ∂ sum(expr_i) / ∂ s_{N-1} ]
```

The implicit cotangent on every primal output is `1.0` (sum cotangent).
Custom output cotangents are reserved for a future `vjp(expr, cotangent,
seeds)` primitive.

Arity contract:

- `body.outputs ≥ 1` (`PropagateError::RadBodyArity`),
- `seeds.outputs ≥ 1` (`PropagateError::RadSeedArity`),
- `inputs = max(body.inputs, seeds.inputs)`,
- `outputs = body.outputs + seeds.outputs`.

## 2. Algorithm

`reverse_ad.rs` performs three explicit passes on a single
`ReverseADTransform` instance.

### 2.1 Active subgraph collection

Postorder DFS from each primal output through the differentiable
children of every visited node. Descent stops at any `SigId` that
appears in the seed list, and DAG sharing is preserved by a `visited`
set so each node is visited at most once.

```text
collect_dfs(root):
  if seen(root):                          return
  if root ∈ seeds:                        record root, return
  for child in active_children(root):
    collect_dfs(child)
  postorder.push(root)
```

`active_children(sig)` reuses the same `match_sig` decoding as the
adjoint emission code, so a node that gets descended in pass (1) is
guaranteed to have a matching adjoint rule in pass (2).

### 2.2 Adjoint accumulation

Initialize each primal's adjoint to `1.0` (the sum cotangent), then
walk the postorder in reverse and emit local transpose contributions:

```text
for primal in primals:
  adjoints[primal] += 1.0

for y in reverse(postorder):
  if y ∈ seeds:                           continue           // leaf
  y_bar = adjoints[y]
  for (child, factor) in transpose_rule(y):
    adjoints[child] += y_bar · factor
```

`add_adjoint(target, contribution)` accumulates into the map: if
`target` already has an adjoint, it builds `old + contribution`;
otherwise it stores `contribution` directly. This is the structural
counterpart of the explicit-zero-fold step that FAD does at
construction time.

### 2.3 Seed extraction

```text
result = primals
for s in seeds:
  result.push(adjoints.get(s).unwrap_or(zero))
```

Repeated seed lanes preserve the same adjoint identity, so
`rad(a*b, (a, a))` yields two gradient lanes that alias the same
computed signal. Absent seeds (those never reached from any primal
output) yield `0.0`.

## 3. Rule table

The transpose rules below mirror the forward rules in `forward_ad.rs`
for every family that admits a causal reverse pass. Notation:
`y` = visited node, `y_bar` = its accumulated adjoint, `child_bar +=
…` = `add_adjoint(child, …)`.

### 3.1 Leaves and discrete operators

| Node | Reverse behaviour |
|------|-------------------|
| `int(c)`, `real(c)` | no children |
| `sigInput(_)` | no children |
| `hslider`, `vslider`, `numentry` (not seed) | no children |
| `button`, `checkbox` | no children (discrete) |
| seed `s` | descent stops; final `adjoints[s]` is the gradient lane |
| comparisons / shifts / bitwise `BinOp` | no contribution |

### 3.2 Arithmetic `BinOp`

| `y = …` | Adjoint contributions |
|---------|-----------------------|
| `x + z` | `x_bar += y_bar`; `z_bar += y_bar` |
| `x - z` | `x_bar += y_bar`; `z_bar += -y_bar` |
| `x * z` | `x_bar += y_bar · z`; `z_bar += y_bar · x` |
| `x / z` | `x_bar += y_bar / z`; `z_bar += y_bar · (-x / z²)` |
| `x % z` | `x_bar += y_bar`; `z_bar += y_bar · -⌊x/z⌋` |

### 3.3 Unary trig/transcendental

The chain rule `child_bar += y_bar · f'(child)` is applied with the
same closed-form derivatives as FAD:

| `y = f(x)` | `f'(x)` |
|-----------|---------|
| `sin(x)` | `cos(x)` |
| `cos(x)` | `-sin(x)` |
| `tan(x)` | `1 / cos²(x)` |
| `exp(x)` | `exp(x)` |
| `log(x)` | `1 / x` |
| `log10(x)` | `1 / (x · ln 10)` |
| `sqrt(x)` | `1 / (2 · √x)` |
| `abs(x)` | `x / |x|` |
| `acos(x)` | `-1 / √(1 - x²)` |
| `asin(x)` | `1 / √(1 - x²)` |
| `atan(x)` | `1 / (1 + x²)` |

At non-differentiable points the emitted formula is not regularized. In
particular, the current `abs` rule can produce `NaN` at `x = 0` because it uses
`x / |x|`.

### 3.4 Binary math

| `y = f(x, z)` | Contribution to `x_bar` | Contribution to `z_bar` |
|---------------|-------------------------|-------------------------|
| `pow(x, z)` | `y_bar · pow(x,z) · z / x` | `y_bar · pow(x,z) · log(x)` |
| `atan2(y_n, x_n)` | `y_bar · -y_n / (x_n² + y_n²)` to `x_n` | `y_bar · x_n / (x_n² + y_n²)` to `y_n` |
| `min(x, z)` | `y_bar` if `x < z`, else `0` | `y_bar` if `x ≥ z`, else `0` |
| `max(x, z)` | `y_bar` if `x > z`, else `0` | `y_bar` if `x ≤ z`, else `0` |
| `fmod(x, z)` | `y_bar` | `y_bar · -⌊x/z⌋` |
| `remainder(x, z)` | `y_bar` | `y_bar · -round(x/z)` |

The branch routing for `min`/`max` is materialized via `select2`; the
condition itself receives no adjoint since it is a discrete branch
selector.

### 3.5 Control flow and casts

| Node | Reverse behaviour |
|------|-------------------|
| `select2(cond, x, z)` | `x_bar += select2(cond, y_bar, 0)`; `z_bar += select2(cond, 0, y_bar)`; `cond` receives nothing |
| `float_cast(x)` | `x_bar += float_cast(y_bar)` |
| `int_cast(x)` | no contribution (discontinuous truncation) |
| `bit_cast(x)` | unsupported representation-level operation; RAD rejects it |

### 3.6 Read-only tables

For `y = rdtbl(T, idx)` where `T` is read-only (a `Waveform` or a
write-once `WrTbl(_, _, nil, nil)`), the table contents are treated as
constant data. RAD differentiates only through the read address using
the same symmetric finite-difference slope as FAD:

```text
y       = rdtbl(T, idx)
slope   = (rdtbl(T, idx + 1) - rdtbl(T, idx - 1)) / 2
idx_bar += y_bar · slope
```

Mutable tables (`WrTbl` with non-nil write ports) refuse adjoint and
raise `RadUnsupportedNode { kind: "writable-table" }`.

### 3.7 Foreign functions

Recognised unary FFun families (precision-agnostic match on the
descriptor name):

| Name | Adjoint |
|------|---------|
| `tanh` | `y_bar · (1 - tanh²(x))` (reuses primal) |
| `sinh` | `y_bar · cosh(x)` rebuilt as `y_bar · √(1 + sinh²(x))` |
| `cosh` | `y_bar · sinh(x)` rebuilt as `y_bar · (e^x - e^{-x}) / 2` |
| `atanh` | `y_bar / (1 - x²)` |
| `asinh` | `y_bar / √(1 + x²)` |
| `acosh` | `y_bar / √(x² - 1)` |

Non-unary or unrecognised FFun calls raise
`RadUnsupportedNode { kind: "ffun" }`.

### 3.8 Pass-through wrappers

`Attach`, `Enable`, `Control`, and `Output` are transparent to
differentiation: the adjoint is forwarded to the signal-carrying
operand only. Bargraphs (`vbargraph` / `hbargraph`) are metering
sinks — they are walked so seed-reachability is correctly classified
but propagate no adjoint.

## 4. Temporal boundary

Forward-mode AD applies a causal rule for delays:

```text
∂ delay1(x) / ∂p = delay1(x')         // tangent at frame n depends on frame n-1
```

Reverse-mode AD requires the transpose, which is anti-causal:

```text
adj_x[n] += adj_y[n + 1]              // adjoint at frame n depends on a future frame
```

A correct reverse pass therefore needs either

- a finite block tape that buffers primal intermediates and a backward
  scan over that block, or
- a causal approximation that is explicitly not exact reverse mode.

Current RAD takes the finite-block route through `SigBlockReverseAD`.
The local symbolic sweep remains feed-forward only: when it reaches a
delay, prefix, recursion, or IIR carrier, it raises
`PropagateError::RadUnsupportedNode` with a kind label. The public
`generate_rad_signals` dispatcher catches the temporal/recursive kinds
and emits a `BlockReverseAD` carrier instead of surfacing the diagnostic.
Hard unsupported families such as mutable tables, soundfiles, and
unrecognized foreign functions still surface targeted diagnostics.

The `BlockReverseAD` lowering evaluates the primal body forward over
the current `compute(count)` block, records the intermediate values it
needs in real-valued BRA tapes, then runs the backward sweep over that
same block. The gradient lanes are per-sample contributions for the
block-local objective; users can sum them over the block or reduce them
in DSP code with a block length such as `ma.BS`.

The plan still reserves `rad(expr, seeds, horizon)` and `-rad-horizon N`
for a future explicit-horizon mode; current BRA semantics use the
current compute block as the finite horizon. RAD must never silently
emit a misleading gradient.

Phase E0 added a read-only classifier in
`crates/propagate/src/stateful_rad.rs` for `DEBRUIJNREC` groups. It
classifies recursive bodies as `LinearLti`, `LinearTimeVarying`, or
`Nonlinear`. This classifier now annotates fallback mode and future
strategy selection; it no longer selects a public `ReverseTimeRec`
fast path.

The same module also exposes `RecRadMode`, a strategy gate for the
next phases:

| Recursive class | Future RAD mode |
|-----------------|-----------------|
| `LinearLti` | `LinearTranspose` (dormant specialized phase E1 path) |
| `LinearTimeVarying` | `BlockLinearTimeVarying` (phase E2) |
| `Nonlinear` | `BpttRequired` (phase F) |

The `ReverseTimeRec` LTI/IIR path remains in the codebase as dormant
helper infrastructure, but public RAD propagation no longer emits it.
Temporal and recursive public RAD outputs use `BlockReverseAD`; the
mode labels are retained to classify what more specialized strategy
could replace BRA later.

The diagnostic kinds are:

| `kind` | Family |
|--------|--------|
| `delay-or-prefix` | `Delay1`, `Delay`, `Prefix` |
| `recursive-linear-transpose` | LTI recursive class for the dormant E1 path |
| `recursive-block-linear-time-varying` | `Proj` over LTV `DEBRUIJNREC` (future E2) |
| `recursive-bptt-required` | `Proj` over nonlinear `DEBRUIJNREC` (future F) |
| `recursive-projection` | recursive fallback when no specific mode was classified |
| `writable-table` / `writable-table-or-waveform-direct` | mutable tables |
| `ffun` | non-unary or unrecognised foreign function |
| `soundfile` | `Soundfile`, `SoundfileLength`, `SoundfileRate`, `SoundfileBuffer` |
| `other` | catch-all (representation casts, generators, opaque) |
| clock-domain kinds | `ondemand`, `upsampling`, `downsampling`, `Seq`, and boundary glue; rejected until a clock-aware reverse tape exists |

Temporal/recursive kinds are normally caught by the public dispatcher
and converted to `BlockReverseAD`. If one of those diagnostics surfaces
directly, it indicates a fallback-dispatch regression. Hard unsupported
kinds still emit structured diagnostics with kind-specific notes and
help text.

## 5. Relationship to FAD

The explicit symbolic rules of both passes overlap for feed-forward
expressions, but their unsupported-family policies differ: FAD generally
preserves the primal with zero tangents, while RAD rejects hard unsupported
families rather than emitting an unverified gradient.

| Property | FAD | RAD (current) |
|----------|-----|---------------|
| Direction | tangent ↑ (per seed) | adjoint ↓ (per primal) |
| Seed model | explicit `(s_0, …, s_{N-1})` | explicit `(s_0, …, s_{N-1})` |
| Output layout | `[p, t_0, …, t_{N-1}]` interleaved per primal | `[primals…, gradient(s_0), …]` flat |
| Cost per added seed | one extra tangent lane per primal node | one gradient extraction lane; the reverse sweep is shared |
| Cost per added primal | one extra dual rebuild | one extra adjoint initialization; active-subgraph traversal remains shared |
| Recursive primals | yes (via `DEBRUIJNREC` interleaving) | yes, via block-local `BlockReverseAD` fallback |
| Delays | yes (causal forward rule) | yes, via block-local `BlockReverseAD` fallback |
| Clock domains | dual rules for valid clocked blocks; runtime tests cover inside/around `ondemand`; clock is opaque | crossing clock-domain machinery is rejected |
| Multi-output cotangent | implicit per-tangent | implicit all-ones (sum cotangent) |

For feed-forward expressions, RAD gradients agree with the
corresponding FAD tangent lanes lane-by-lane (scalar primal) or with
the **sum** of FAD tangent lanes across primals (multi-output, due to
the implicit all-ones cotangent). This identity is pinned by the
parity tests in `crates/compiler/tests/rad_runtime.rs`.

## 6. Test surface

- **Structural** ([crates/propagate/tests/core_api.rs](../crates/propagate/tests/core_api.rs))
  — arity contract, feed-forward success, temporal/recursive
  `BlockReverseAD` fallback, diagnostic content checks for hard
  unsupported families and internal kind labels.
- **Runtime parity** ([crates/compiler/tests/rad_runtime.rs](../crates/compiler/tests/rad_runtime.rs))
  — RAD vs FAD parity, RAD vs central finite differences, repeated /
  absent seeds, multi-output sum cotangent, read-only table index,
  supported unary FFun families (`tanh`, `sinh`, `cosh`, `atanh`,
  `asinh`, `acosh`), and recursive BRA cases.
- **Backend parity** ([crates/compiler/tests/signal_fir_lane.rs](../crates/compiler/tests/signal_fir_lane.rs))
  — C, C++, interpreter, and Cranelift lowering of RAD/BRA shapes within the
  current fast-lane subset.
- **Corpus** ([tests/corpus/rad_*.dsp](../tests/corpus)) — fixtures
  pin the source-level shape of each contract: arithmetic, trig
  composition, multi-seed, multi-output, repeated/absent seeds,
  read-only table indexing, accepted recursive/block RAD forms,
  plus arity error fixtures (`err_rad_zero_body`, `err_rad_zero_seed`) and
  the temporal fallback fixture `rad_delay1_block_fallback`.

## 7. Out-of-scope and future work

The following remain explicitly out of scope for current RAD:

- specialized `ReverseTimeRec` / phase-E recursive fast paths in public RAD
  dispatch,
- explicit user-controlled horizons (`rad(expr, seeds, horizon)` /
  `-rad-horizon N`),
- adjoints over mutable tables,
- adjoints over soundfile content,
- custom vector-output cotangent API (`vjp(...)`),
- backend-level Enzyme/LLVM integration,
- automatic discovery of differentiable UI controls.
- reverse mode across `ondemand` / `upsampling` / `downsampling` boundaries.

Plan phases E and F sketch the next steps:

- **Phase E0** — implemented read-only recursive-linearity classifier
  and `RecRadMode` strategy gate; no new `rad(...)` capability.
- **Phase E1/E2** — scoped specialized recursive strategies that can replace
  generic BRA where profitable and proven equivalent.
- **Phase F** — a finite-horizon BPTT mode (`rad(expr, seeds, horizon)`
  or `-rad-horizon N`) requiring a runtime tape and a backend backward
  sweep.

These phases are gated separately and require explicit documentation
of latency and memory footprints before merge.

## 8. Source locations

- `generate_rad_signals` and `ReverseADTransform`:
  [crates/propagate/src/reverse_ad.rs](../crates/propagate/src/reverse_ad.rs)
- Propagation arm and arity contract:
  [crates/propagate/src/lib.rs](../crates/propagate/src/lib.rs) — search
  for `FlatNodeKind::ReverseAD`.
- `RadBodyArity` / `RadSeedArity` / `RadUnsupportedNode` diagnostics:
  [crates/propagate/src/error.rs](../crates/propagate/src/error.rs), in the
  `ToDiagnostic` implementation for `PropagateError`.
- `BlockReverseAD` FIR lowering:
  [crates/transform/src/signal_fir/block_reverse_ad.rs](../crates/transform/src/signal_fir/block_reverse_ad.rs).
- Stateful RAD feasibility classifier:
  [crates/propagate/src/stateful_rad.rs](../crates/propagate/src/stateful_rad.rs).
- Implementation plan:
  [porting/reverse-ad-rad-implementation-plan-2026-04-27-en.md](../porting/reverse-ad-rad-implementation-plan-2026-04-27-en.md).
