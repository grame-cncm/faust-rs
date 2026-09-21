---
title: "What a seed means when it depends on another seed: the semantics of `fad` and `rad`, the alternatives, and what other frameworks do"
date: 2026-09-21
page-size: A4
margins: 20 22
page-numbers: true
font-body: Roboto
font-heading: Roboto Condensed
font-mono: Roboto Mono
---

**Date:** 2026-09-21

**Status:** analysis and recommendation; not a normative plan. Nothing in the
compiler changes with this document.

**Studied tree:** `main-dev` at `194f7b12`. Code read:
`crates/propagate/src/forward_ad.rs` (seed index, `depends_on_seed`,
`transform_uncached`, the `Proj` arm), `crates/propagate/src/reverse_ad.rs`
(`collect_dfs`, `run`), `crates/transform/src/signal_fir/block_reverse_ad.rs`
(the block sweep's stop set), `crates/propagate/src/engine.rs` (the lowering
of the seed box), `libraries/optimizers.lib` (the clocked descents).

**Related documents:** [`../docs/fad-note-en.md`](../docs/fad-note-en.md),
[`../docs/rad-note-en.md`](../docs/rad-note-en.md),
[`../docs/fad-rad-synthesis-en.md`](../docs/fad-rad-synthesis-en.md),
[`journal/2026-09-21.md`](journal/2026-09-21.md).

## 1. The question

```faust
process(x, y) = fad(x + y, (x + y, x, y));
```

The program has four outputs: the primal `x + y` and three tangents, one per
seed. The seeds are the expression `x + y` itself, then `x`, then `y`. Both
compilers of this tree give

| lane | seed | `fad` today | `rad` today | expected by the reporter |
|---|---|---|---|---|
| 1 | `x + y` | `1` | `1` | `1` |
| 2 | `x` | `0` | `0` | `1` |
| 3 | `y` | `0` | `0` | `1` |

The zeros are not an accident of one mode: `fad` and `rad` agree, and the
result does not depend on the order of the seeds (`(x, y, x + y)` gives
`0, 0, 1`). Two more probes, both modes again equal:

| program | seeds | today |
|---|---|---|
| `fad(2 * (x + y), …)` | `(x + y, x)` | `2, 0` |
| `fad(x + y, …)` | `(x, y)` | `1, 1` |

So the question is not a bug in a rule. It is what a seed means when it is
computed from another seed. The current answer is one of three defensible
ones, and this document sets them side by side, with what each does to the
programs faust-rs is used for.

## 2. What the two modes do today

### 2.1 Forward mode

A seed is recognised by `SigId` identity: the arena hash-conses every node, so
the `x + y` of the body and the `x + y` of the seed list are one `SigId`. In
`ForwardADTransform::transform_uncached` the first test on every node is
whether the node is a seed; if it is, the node's dual is its primal with a
unit tangent on its own lanes and zero on every other, and **the transform
returns without visiting the node's operands**:

```rust
if let Some(seed_slots) = self.diff_seed_index.get(&sig).cloned() {
    let one = SigBuilder::new(self.arena).real(1.0);
    let mut tangents = self.zero_tangent_lanes_real();
    for slot in seed_slots { tangents[slot] = one; }
    return Dual { primal: sig, tangents };
}
```

Three consequences follow, all deliberate in the file's own comments:

- a seed is a *leaf* of the differentiated graph; whatever computes it (a
  clamp, an initialisation gate, a recursion) is not entered and gets no
  tangent lanes;
- duplicated seeds (`(s, s)`) give `1` on both lanes, because the index maps
  one `SigId` to several slots;
- a seed that is a projection of a recursion (`prev` in a `~` loop) has
  tangent exactly `1`, not the sensitivity of the state to itself through
  its history. `depends_on_seed` then decides which recursions the transform
  augments: a recursion is rewritten with `1 + N` interleaved lanes only if
  its body contains a seed. A recursion *upstream* of a seed, reached only
  through the seed, is not augmented.

### 2.2 Reverse mode, symbolic sweep

`ReverseADTransform::collect_dfs` builds the postorder from each primal and
**stops its descent at any seed**; `run` then walks the postorder backwards,
accumulates `child_bar += y_bar · ∂y/∂child`, and skips seeds (`continue`)
so no adjoint flows below them. The gradient lane of a seed is its
accumulated adjoint, `0.0` when no primal reaches it. The rule table of
`rad-note-en.md` §3 states it: "seed `s`: descent stops; final `adjoints[s]`
is the gradient lane".

### 2.3 Reverse mode, block sweep

`block_reverse_ad.rs` keeps the same contract for the temporal fallback. The
postorder of the body is built with a stop set holding the seeds, and the
comment says why:

> A seed is a leaf of the differentiated graph, exactly as in the symbolic
> reverse sweep: its adjoint *is* the gradient lane, and what computes the
> seed (a clamp, a `select2` initialisation, the optimizer's own recursion)
> is outside the loss. Descending into it would both reject node kinds the
> backward pass has no business seeing and leak adjoint carries into the
> enclosing recursion.

That last clause is the load-bearing one for this analysis, and §5 returns
to it.

### 2.4 In one sentence

**Today a seed is detached: the derivative is taken as if every seed were an
independent variable, and the computation of a seed is a constant for every
lane.** Put plainly: a signal listed as a seed becomes an unknown of its own,
and the compiler forgets how it was computed. In the reporter's program,
`x + y` is a third unknown `u`; the body is `u`; `∂u/∂u = 1`, `∂u/∂x = 0`,
`∂u/∂y = 0`. Two questions hide in that program, and each has its own
spelling: seed `(x, y)` to ask how `x + y` moves with `x` and `y` (`1, 1`);
seed `x + y` alone to ask how the output moves with that quantity (`1`).

## 3. Three candidate semantics

Let the seeds be `s_1 … s_N`, the body `f`. Write the answer of lane `j` for
each candidate.

**A. Detached seeds** (current). Lane `j` is `∂f/∂s_j` in the coordinate
system where the `N` seeds are independent variables; a seed's own
computation is constant. Equivalent formulation: every seed is wrapped in a
stop-gradient before differentiation, then differentiated with respect to.

**B. Own lane cut, other lanes pass.** The tangent of seed `s_j` is `1` on
lane `j` (exactly, no accumulation through its own history) and, on every
other lane `k`, what its operands propagate. Lane `j` of the output is the
sensitivity to a perturbation injected at `s_j`, flowing downstream through
everything, other seeds included; but a perturbation never flows *through*
its own seed.

**C. Injected perturbation** (the "adjoint of an intermediate node"). Lane
`j` is the sum over all paths from the outputs back to `s_j` of the products
of local derivatives, with no stop anywhere. In reverse mode this is the
accumulated adjoint of node `s_j` with the sweep continuing below every
seed; in forward mode the tangent of `s_j` is `1` plus whatever its operands
carry, on every lane including its own.

On the reporter's program A gives `1, 0, 0`; B and C give `1, 1, 1`.

### 3.1 The programs that tell them apart

The three agree wherever no seed is computed from another seed and no seed
is computed from itself through time. They part on five shapes, each with
a program in the corpus or the library.

**P1. A seed that is a stateless function of other seeds.**
`fad(x + y, (x + y, x, y))`. A: `1, 0, 0`. B, C: `1, 1, 1`.

**P2. A control feeding a recursive filter, seed the control.**
`fad(+ ~ *(fb) : *(vol), fb)` (`fad_recursive_multi_control.dsp`). The
tangent of the state accumulates through the recursion: `dy[n]/dfb = y[n-1]
+ fb · dy[n-1]/dfb`. All three agree: `fb` has no operands, so there is
nothing to cut or to pass. The result is the
sensitivity to a *persistent* change of `g`, the same `g` at every sample,
which is what a parameter is.

**P3. The in-graph descent, seed the state itself.**
`fad_recursive_local_projection.dsp`:

```faust
process = step ~ _ with {
    step(prev) = prev - lr * grad with {
        loss = (prev - target) ^ 2;
        grad = fad(loss, prev) : !, _;
    };
};
```

The seed `prev` is `Proj(0, REC)`. A and B: tangent of `prev` is `1`, the
gradient is `2 (prev - target)`, the one-step gradient a descent wants. C:
the tangent of `prev` is `1` plus what its operand, the recursion carrier,
carries on the same lane, that is `d prev[n] / d prev[n-1] · tangent[n-1]`;
the lane becomes the sensitivity of the state to a perturbation applied at
every sample of itself, an infinite-horizon sum. The descent no longer
computes a gradient. **C breaks every in-graph optimizer of the library.**

**P4. Two parameters of one optimizer recursion, seeds both.**
`descend_2D_clocked` in `optimizers.lib`:

```faust
loop(prev1, prev2) = … with {
    p1 = clip(lo1, hi1, init1 + _dev_clocked(clock, reset, prev1));
    p2 = clip(lo2, hi2, init2 + _dev_clocked(clock, reset, prev2));
    gs = fad(loss(p1, p2), (p1, p2)) : !, _, _;
    …
};
```

Each seed is a clamp of a `select2` of a projection of the loop. A: lane 2 is
`∂loss/∂p2` with `p1` fixed, the partial derivative a gradient step needs;
the loop is not augmented, because nothing inside it is a seed. B: the
tangent of `p1` on lane 2 is what its operands carry on lane 2, `d p1[n] /
d p2` through the loop's history, since `prev1[n]` depends on the gradient
at `n-1`, which depends on `p2[n-1]`; lane 2 becomes `∂loss/∂p2 + ∂loss/∂p1
· dp1/dp2`, a total derivative along the optimizer's own trajectory, which
is not a gradient of the loss; and to compute it the transform must augment
the optimizer's recursion with `N` lanes, the cost the block sweep's comment
calls "leaking a carry into the enclosing recursion". C does the same and
adds the P3 accumulation. **B and C both change every multi-parameter
descent of the library, in value and in cost.**

**P5. Duplicated seed.** `fad(f, (s, s))`. A, B, C: `1` on both lanes; no
difference.

The table:

| | P1 stateless composite | P2 control into filter | P3 state as seed | P4 optimizer parameters | P5 duplicate |
|---|---|---|---|---|---|
| A (today) | `1, 0, 0` | exact | one-step gradient | partial derivatives, loop untouched | `1, 1` |
| B | `1, 1, 1` | exact | one-step gradient | trajectory derivative, loop augmented | `1, 1` |
| C | `1, 1, 1` | exact | history accumulation | trajectory derivative, loop augmented | `1, 1` |

P1 is the reporter's program; P3 and P4 are what faust-rs's differentiation
exists for. Only A serves both P3 and P4. B and C fix P1 by breaking P4.

### 3.2 Why reverse mode makes B harder than it looks

Forward mode carries one tangent per lane, so B ("cut on the own lane, pass
on the others") is a local rule at the seed node. Reverse mode carries one
adjoint per node. The path from the output through a projection seed `s_j`
into its history is the path B must cut for lane `j` and must keep for a
lane `k` whose seed sits upstream in that history. One adjoint cannot be cut
for one lane and kept for another. B in the block sweep needs either one
sweep per seed that is a state, or adjoints tagged by origin lane. The
symbolic sweep, feed-forward by construction, has no such path, and there B
is exactly "do not stop at seeds", a two-line change. The two modes would
then agree on feed-forward programs and disagree on recursive ones unless
the harder work is done. Today they agree everywhere, and the corpus
(`rad_fad_multi_seed.dsp`) tests that agreement.

## 4. What other frameworks do

None was run for this document (none is installed on this machine); the
statements below are the frameworks' documented contracts.

**PyTorch.** `torch.autograd.grad(outputs, inputs)` accepts any tensor of the
graph as an input, intermediate tensors included, and returns the
accumulated adjoint of each; the backward pass does not stop at an input, so
with `u = x + y`, `grad(u, [u, x, y])` gives `(1, 1, 1)`. That is C. But a
PyTorch training loop never differentiates through a parameter's own update:
parameters are leaves, the optimizer step runs under `torch.no_grad()`, and
`detach()` exists for anything else. In other words PyTorch computes C on a
graph where the user has already applied A at the parameters by construction
of the API, and where every time step is a distinct tensor, so "the state"
as a seed means one step's tensor, never the whole trajectory. Faust has
neither property: a signal is all its samples, and the update is in the
graph. The seed cut is what stands in for `no_grad` and for per-step tensor
identity.

**TensorFlow.** `tf.GradientTape.gradient(target, sources)` likewise accepts
watched intermediates as sources and lets the adjoint flow below them: C,
with `tf.stop_gradient` as the explicit cut. Variables updated by an
optimizer are not differentiated through, as in PyTorch.

**JAX.** `jax.grad`, `jvp`, `vjp` differentiate a function with respect to
its *arguments* only. An intermediate cannot be a seed; to differentiate
with respect to `u = x + y` one writes `g(u, x, y)` and passes `u` in, at
which point `x` and `y` no longer reach the output through `u`: the answer
is `(1, 0, 0)`, **A**. JAX therefore never faces the question; its API makes
the seeds independent by construction, and `lax.stop_gradient` cuts inside
the body. Zygote (`gradient(f, args…)`), Enzyme (activity annotations on
arguments) and ForwardDiff (dual numbers seeded on the inputs, a `Dual` for
`x + y` carrying partials `(1, 1)` whether one likes it or not) are in the
same family: seeds are function inputs, which cannot depend on one another.

**The AD literature.** Griewank and Walther define the independent
variables as the inputs of the evaluation trace and give every
intermediate `v_i` an adjoint `v̄_i = ∂y/∂v_i`, the sensitivity of the
output to a perturbation *injected* at `v_i` with everything upstream
unchanged. Seeding a tangent at an intermediate is C. Differentiating "with
respect to an intermediate as an independent variable" is A and is a change
of variables, only meaningful if the intermediate is made a free input, as
JAX forces.

**Where faust-rs sits.** Its seed list is PyTorch's `inputs` in surface (any
signal may be named) with JAX's semantics underneath (the named signals are
treated as independent inputs). The mixture is what makes P1 surprising: a
PyTorch user reads `(x + y, x, y)` as three tensors and expects `1, 1, 1`; a
JAX user cannot write it. The mixture is also what makes P3 and P4 work
without any `detach`, because the cut is built into the seed.

**Stop-gradient.** Every framework above has one: `detach`,
`tf.stop_gradient`, `lax.stop_gradient`, `ChainRulesCore.ignore_derivatives`,
Enzyme's constant activity. faust-rs has none; the seed cut is one surrogate,
and the integer cast used by faust-diff-demo's leader/scout program
(`float(int(v * 2^20)) / 2^20`, so that a `fad` does not differentiate the
scout through the leader's held starts) is the other.

## 5. The corpus

A scan of every `fad(` and `rad(` call in `tests/corpus`, `tests/**`,
`tests/impulse-tests/dsp-rad` and `libraries/*.lib` (212 calls) found no
program whose seed is computed from another seed. The 24 calls whose seed
text contains an operator or parentheses are all sliders spelled inline
(`hslider("f", …)`), and the library's descents seed the parameters, which
depend on each other only through the optimizer's recursion, the P4 shape.
The `faust-diff-amp`, `-ts808`, `-jot`, `-rir` and `-demo` programs seed
their parameters the same way. So:

- A change to B or C would alter no output of the corpus by P1, and would
  alter every multi-parameter descent by P4;
- no test today guards P1 in either direction.

## 6. Recommendation

**Keep A**, and make it explicit.

The reason is P3 and P4. The whole in-graph learning surface of faust-rs,
`optimizers.lib` and the four faust-diff projects included, rests on a
parameter's gradient being the partial derivative with the other parameters
fixed and with the optimizer's own recursion outside the differentiated
graph. That is exactly the detachment A provides, and it is what the other
frameworks provide too, by other means: leaves and `no_grad` in PyTorch,
arguments in JAX. B and C would require every parameter of every descent to
be wrapped in a stop-gradient that faust-rs does not yet have, and would
augment the optimizer recursions with `N` tangent lanes for a total
derivative nobody asked for.

The earlier answer given in conversation on this day, that `1, 1, 1` was
the more reasonable semantics, was reached on P1 alone. P3 and P4 reverse
it. A is not a coordinate artefact once one sees that faust-rs's seeds are
JAX arguments, not PyTorch tensors.

Three things should follow, in order of value.

**6.1 A diagnostic for dependent seeds.** A seed computed from another seed
is, in every program seen, a mistake, and its symptom is a silent zero. At
the lowering of the seed box (`engine.rs`, the `ForwardAD` and `ReverseAD`
arms, after `seed_sigs` is known), walk each seed's subtree once, memoised,
and report a seed whose subtree contains a *different* seed (`(s, s)` stays
legal). One `PropagateError` variant for both modes, at `Severity::Error`
since no valid program in the corpus does this, with the fix in the
`faust-error-model` form: "seed `x + y` is computed from seeds `x` and `y`;
a seed is differentiated as an independent variable, so lanes `x` and `y`
do not pass through it; drop `x + y` from the seed list, or seed it alone".
An error rather than a warning because a program relying on the zeros on
purpose has a clearer spelling available (seed `u` alone). A corpus fixture
`err_fad_dependent_seeds.dsp` and its `rad` twin; a line in both notes.

**6.2 A stop-gradient primitive.** `detach(x)` (name to be chosen; the C++
compiler has no equivalent): primal `x`, tangent zero in forward mode, no
adjoint below it in reverse mode, transparent to every other pass. It
replaces the integer-cast surrogate, gives P4-style programs an explicit
spelling if they ever want a parameter *not* detached, and is the
precondition for any later injection-mode variant. It was already asked for
in faust-diff-demo's note of 2026-09-21.

**6.3 Later, if a use case appears, an injection mode as an option**, not a
change of default: `fad` and `rad` with C semantics on feed-forward bodies
(forward mode: do not short-circuit at a seed, add `1` on its lanes after
differentiating its operands; symbolic reverse: do not stop at seeds), and
rejected or restricted to leaf seeds on recursive bodies until per-lane
adjoints exist (§3.2). The use case would be a sensitivity analysis with
respect to an intermediate quantity, which today is written by seeding that
quantity alone. None is known.

**What the reporter's program should say.** To get `1, 1` with respect to
`x` and `y`, seed `(x, y)`; `x + y` is a function of them and its lane is
redundant. To get the sensitivity to `x + y` as a quantity, seed it alone.
The two questions have different answers and A makes the program choose.

## 7. Where the notes need a sentence

- `docs/fad-note-en.md` §2, after "Repeated seed lanes are preserved": a
  seed is differentiated as an independent variable; its own computation
  is a constant for every lane, so a seed computed from another seed does
  not pass that seed's tangent (with the P1 example and its two spellings).
- `docs/rad-note-en.md` §3.1, the "seed `s`" row: the same sentence in
  adjoint terms.
- `docs/fad-rad-synthesis-en.md` and `-fr.md`, "Practical limits": one
  bullet.
- `libraries/optimizers.lib` header: that the descents rely on this, which
  is why no parameter needs detaching.
