# RAD/BRA Analysis: Tape Safety, Adjoint Liveness, and Routing

**Date:** 2026-07-27

**Status:** analysis confirmed, remediations proposed, implementation deferred

**Analyzed baseline:** `de7667be` (`Fix BlockReverseAD attach adjoint lowering`)

**Production path:**
`parse -> boxes -> eval -> propagate::reverse_ad -> signal_prepare ->
signal_fir::BlockReverseAD -> backend`

## 1. Executive Summary

Commit `de7667be` gives `Attach(value, effect)` the correct semantics in the
`BlockReverseAD` backward pass:

- `effect` executes only in the forward pass;
- the cotangent is forwarded only to `value`;
- the bargraph is neither differentiated nor replayed in the backward pass.

Inspection of the resulting C++ exposes three broader problems:

| Finding | Verdict | Impact |
|---|---|---|
| `i0 & 8191` protects the tape when `count > 8192` | protects memory only | silently incorrect gradients |
| a tape can be declared and written without any load | confirmed | unnecessary DSP memory and real-time stores |
| `rad` always takes the `BlockReverseAD` path | false in general | this case falls back because seed-independent subgraphs are not pruned |

The key distinction is:

> The **primal** graph of `no.noise` is temporal, but the **useful adjoint
> subgraph for seed `w`** in `w * no.noise` has no temporal edge.

The RAD dispatcher currently examines the former. It should examine the latter
before deciding that a temporal tape is required.

## 2. Reproducer and Observed Result

The standalone reproducer is:

```faust
import("stdfaust.lib");

w = hslider("w", 0.5, -1, 1, 0.001);
x = attach(no.noise, no.noise : hbargraph("probe", -1, 1));

process = rad(w*x, w);
```

Its Signal IR contains:

```text
BlockReverseAD(
    body  = Mul(w, Attach(noise_rec, HBargraph(noise_rec))),
    seeds = [w],
    cotangents = [1]
)
```

The relevant double-precision C++ has this shape:

```cpp
double fBraTape0[8192];
double fBraTape1[8192];

for (int i0 = 0; i0 < count; ++i0) {
    // Primal noise computation.
    double fTemp0 = /* promoted integer generator state */;
    double fTemp1 = /* normalized noise */;
    fHbargraph1 = FAUSTFLOAT(fTemp1);

    int iTemp0 = i0 & 8191;
    fBraTape0[iTemp0] = fTemp0;
    fBraTape1[iTemp0] = fTemp1;
    output0[i0] = FAUSTFLOAT(w * fTemp1);
}

for (int i0 = count - 1; i0 >= 0; --i0) {
    output1[i0] = FAUSTFLOAT(1.0 * fBraTape1[i0 & 8191]);
}
```

The generated gradient is correct when `count <= 8192`:

```text
∂(w * x) / ∂w = x
```

The bargraph store is also correctly placed in the forward pass.

## 3. Finding A — The 8192 Limit Remains a Correctness Cliff

### 3.1 Current State

`MAX_BRA_TAPE_BLOCK_SIZE` is 8192, and each tape is a fixed array in the DSP
struct. `bra_tape_index` computes:

```text
i0 & (MAX_BRA_TAPE_BLOCK_SIZE - 1)
```

This mask removed the historical out-of-bounds access, but it did not remove
the precondition:

```text
count <= MAX_BRA_TAPE_BLOCK_SIZE
```

### 3.2 Exact Corruption Pattern

For `count = 9000`:

1. the forward pass writes samples `0..8191` into slots `0..8191`;
2. samples `8192..8999` overwrite slots `0..807`;
3. the backward pass reads the end of the block correctly;
4. when it reaches samples `0..807`, it reads values from `8192..8999`.

After one wrap, the prefix of length `count - 8192` is therefore corrupted.
For still larger blocks, several generations of values alias in the same
slots.

This failure mode is particularly dangerous:

- no compilation error;
- no invalid memory access;
- no runtime diagnostic;
- plausible primal outputs;
- incorrect gradients only.

### 3.3 Proposed Remediation: Make the TBPTT Window Explicit

Two contracts are possible. Choosing between them changes the public semantics
of `compute(count, ...)` for `count > W` and must be approved before
implementation.

#### Recommended Option — Fixed, Chunked Window

Introduce an explicit BRA window `W`, with `W <= 8192`, and generate:

```text
for each host chunk [base, min(base + W, count)):
    forward(chunk)
    backward(chunk)
    reset adjoint carries at the chunk boundary
```

The contract becomes:

```text
TBPTT(W, W), independent of the host block size
```

Advantages:

- fixed and bounded memory;
- no allocation inside `compute`;
- applicable to the C, C++, Rust, WASM, and interpreter backends;
- defined behavior for every valid `count`;
- memory cost controlled by a compilation option.

Compatibility:

- unchanged results for `count <= W`;
- for `count > W`, replaces the currently corrupted result with explicit
  truncation at chunk boundaries;
- lowering reports and documentation must expose `W`.

#### Alternative — Dynamic Tape Sized to `count`

Sizing the tape to the host block preserves the currently stated
`TBPTT(count, count)` semantics exactly.

This option requires a new scratch-memory contract:

- no allocation or capacity growth in the real-time loop;
- capacity supplied by the architecture or prepared before `compute`;
- a representation shared by all backends;
- an observable error policy when capacity is insufficient.

It must not be approximated by hidden allocation inside `compute`.

### 3.4 S0 Deliverables and Acceptance Criteria

Deliverables:

1. a documented decision between a fixed chunked window and dynamic scratch;
2. an explicit capacity/window representation in the FIR plan;
3. removal of masked indexing as the sole safety policy;
4. runtime tests at `W-1`, `W`, `W+1`, and `2W+17`.

Pass criteria:

- no backward sample reads a forward slot from another window;
- results agree with an independent TBPTT executor;
- interpreter parity between `opt_level=0` and `opt_level=max`;
- no change for blocks with `count <= W`;
- lifecycle remains conformant: no ad hoc allocation or initialization in the
  sample loop.

## 4. Finding B — Tape Collection Is Not Adjoint-Liveness-Aware

### 4.1 Cause

`ensure_bra_tape_stores` currently:

1. builds the complete postorder of the primal body;
2. calls `collect_tape_needed_values` on that postorder;
3. immediately declares and writes one tape for each selected value.

Its `seed_sigs` and `cotangent_sigs` parameters are unused. Collection therefore
knows that a local differentiation rule *might* need a primal operand, but it
does not know whether the corresponding contribution can reach a requested
seed.

In the reproducer:

- the contribution from `Mul(w, x)` to `w` needs `x`, so `fBraTape1` is useful
  on the current BRA path;
- the contribution to `x` cannot reach a seed because `x` does not depend on
  `w`;
- traversal into the noise generator nevertheless creates intermediate tape
  requirements;
- the pure adjoint computation that would have loaded `fBraTape0` is later
  removed;
- the `fBraTape0` store survives because it has already been materialized as a
  FIR effect.

### 4.2 Proposed Remediation: `BraAdjointPlan`

Before emitting any FIR, build a canonical plan containing:

```text
BraAdjointPlan {
    live_nodes,
    live_adjoint_edges,
    required_primal_values,
    temporal_carries,
    tape_values,
}
```

An adjoint edge `parent -> child` is live only if `child` can reach at least one
requested seed. Primal requirements are then derived only from live rules:

```text
Mul(w, x), seed = w:
    live contribution: adj[w] += adj[y] * x
    dead contribution: adj[x] += adj[y] * w
    required primal value: x only
```

Tapes must be emitted from `required_primal_values`, never from the primal
postorder alone.

### 4.3 Plan Invariants

The plan producer must guarantee:

1. every declared tape has at least one backward load;
2. every backward load has exactly one compatible forward store;
3. the tape type matches the real type consumed by its rule;
4. no seed-independent operand is traversed to produce a dead adjoint;
5. tape ordering is deterministic;
6. temporal carries remain distinct from primal-value tapes.

This plan is a finite structural artifact whose errors can silently produce
incorrect gradients. It therefore warrants the producer/checker assurance
level:

- canonical `BraAdjointPlan` serialization;
- a Rust checker independent of the emitter;
- negative mutations of edges, stores, loads, types, and windows;
- no formal-proof claim unless the checker is itself connected to the Lean
  specification described by the certified-porting plan.

### 4.4 S2 Deliverables and Acceptance Criteria

Deliverables:

1. a `BraAdjointPlan` type in the `signal_fir` subsystem;
2. a seed-aware producer;
3. an independent checker;
4. FIR lowering that consumes only an accepted plan;
5. a structural report of tape count and size.

Pass criteria:

- a forced BRA case `Mul(seed, independent_temporal_value)` produces exactly
  one tape;
- there is no write-only tape in final FIR;
- every negative certificate mutation is rejected;
- plan output is deterministic;
- existing temporal and recursive BRA tests do not regress.

## 5. Finding C — The BRA Fallback Is Too Conservative Here

### 5.1 `rad` Does Not Always Route Through BRA

The current dispatcher tries, in order:

1. `ReverseADTransform`, the feed-forward symbolic sweep;
2. `BlockReverseAD`, only when a supported temporal or recursive node triggers
   fallback.

Feed-forward tests such as `rad(2*x, x)` already use the first path.

### 5.2 Why the Reproducer Falls Back

`ReverseADTransform::collect_dfs`:

- starts from every primal output;
- descends through every differentiable child;
- stops at an exact seed;
- does not first determine whether a child depends on any seed.

For `Mul(w, Attach(noise, meter))`, traversal therefore reaches the recursive
projection in `noise`. That projection triggers BRA fallback even though noise
is independent of `w`.

### 5.3 Proposed Remediation: Seed-Dependency Masks

Compute a mask for every `SigId`:

```text
seed_dependencies(sig) -> BitSet<seed_lane>
```

Core rules:

- an exact seed carries its own bit;
- a non-seed leaf carries the empty mask;
- an operator combines the masks of its differentiable children;
- `Attach(value, effect)` inherits only the mask of `value`;
- `HBargraph` and `VBargraph` are sinks;
- recursive groups are solved by a fixpoint over their slots, rather than by
  a naive DFS that could treat a back-edge as independent.

The symbolic sweep must treat a subgraph with an empty mask as an **opaque
primal value**:

- local Jacobian formulas may still use its value;
- traversal does not construct its own adjoint;
- a seed-independent delay or recursion does not trigger BRA.

For the reproducer:

```text
deps(w)       = {w}
deps(noise)   = {}
deps(w*noise) = {w}
```

The symbolic result becomes:

```text
primal  = w * Attach(noise, meter)
grad_w  = noise
```

Both outputs can be computed in forward time, so no BRA tape is required. The
bargraph store remains part of the original primal.

### 5.4 S1 Deliverables and Acceptance Criteria

Deliverables:

1. a memoized seed-mask analysis;
2. an explicit recursion-group fixpoint;
3. pruning in both DFS collection and adjoint contribution emission;
4. a counter/report explaining why symbolic RAD or BRA was selected.

Pass criteria:

- the reproducer no longer contains `SigBlockReverseAD` after propagation;
- generated C++ retains the `fHbargraph*` store;
- generated C++ contains no `fBraTape*`;
- the gradient remains numerically equal to `noise`;
- recursion that actually depends on a seed still falls back to BRA;
- repeated and multi-lane seeds preserve layout and values;
- complexity remains close to
  `O(V * ceil(seed_count / word_bits))`, measured on the large multi-seed RAD
  corpus before adoption.

## 6. Recommended Implementation Order

```text
S0 — define the window contract and remove silent corruption
  ↓
S1 — compute seed dependency before symbolic/BRA dispatch
  ↓
S2 — build and check a live BraAdjointPlan
  ↓
S3 — optionally optimize explicitly constructed BRA carriers
```

S1 removes every tape from the normal reproducer. S2 remains necessary for
genuine BRA circuits and for carriers constructed directly by internal APIs.

S3 may recognize an explicit `SigBlockReverseAD` whose live plan has no
temporal edge and lower it as a forward computation. This optimization is not
required to fix public `rad` routing and must not precede S1/S2.

## 7. Phase 0 Gate Before Implementation

The applicable Phase 0 checks are:

| Gate | Status / action |
|---|---|
| Production pipeline | confirmed by the reproducer and `--dump-sig` |
| Differential baseline | add the standalone reproducer and `count > W` cases |
| `gGlobal` decomposition | no new global state; analyses owned by the transform/lowerer |
| `TreeArena` performance | memoization required; benchmark the large multi-seed RAD corpus |
| API/lifecycle | choose fixed window vs dynamic scratch before changing `compute` |

No deep implementation should begin before the S0 decision because it fixes
the temporal semantics, memory layout, and backend contract.

## 8. Non-Goals

This proposal does not require:

- another change to `Attach` semantics;
- removal of `SigBlockReverseAD`;
- immediate reactivation of the `ReverseTimeRec` fast path;
- treating every recursion as seed-independent;
- dynamic allocation in `compute` without an architecture contract;
- presenting standard tests as a formal proof.

## 9. Files Affected by a Future Implementation

| File | Responsibility |
|---|---|
| `crates/propagate/src/reverse_ad.rs` | seed-aware dependency and dispatch |
| `crates/propagate/src/stateful_rad.rs` | recursive classification/fixpoint reuse |
| `crates/transform/src/signal_fir/block_reverse_ad.rs` | postorder and value requirements |
| `crates/transform/src/signal_fir/module/bra.rs` | plan, stores, loads, and carries |
| `crates/transform/src/signal_fir/module/build.rs` | optional chunked driver |
| `crates/compiler/tests/block_reverse_ad.rs` | oracle and runtime tests |
| `crates/compiler/tests/rad_runtime.rs` | recursion, optimization, and finite differences |

## 10. Related Documents

- [`signal-to-fir-transform-analysis-2026-06-20-en.md`](signal-to-fir-transform-analysis-2026-06-20-en.md)
  — original W5 finding;
- [`signal-to-fir-rewriting-calculus-2026-06-20-en.md`](signal-to-fir-rewriting-calculus-2026-06-20-en.md)
  — hidden `count` precondition;
- [`rad-linearize-once-transpose-plan-2026-05-21-en.md`](rad-linearize-once-transpose-plan-2026-05-21-en.md)
  — long-term residual and unzip/tape direction;
- [`rad-block-reverse-ad-signal-ir-plan-2026-05-07-en.md`](rad-block-reverse-ad-signal-ir-plan-2026-05-07-en.md)
  — `SigBlockReverseAD` carrier contract.
