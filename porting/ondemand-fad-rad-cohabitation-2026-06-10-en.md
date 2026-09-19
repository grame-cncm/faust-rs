# Cohabitation of FAD/RAD with Clock Domains

Date: 2026-06-10

Status: proposed

Implementation clarification (2026-09-19): §6 now distinguishes an augmented
block's shared primal/tangent domain from the separate domain of an original
block still consumed outside `fad`. The initial domain-reuse assumption was
invalid in that case. See the [current implementation explanation and
review](fad-clock-domains-explanation-and-review-2026-09-19-en.md) for the
invariant, tests, remaining variable-`ZeroPad` defect, and assurance limits
at `f18a50c8`. Historical descriptions
of "current behavior" below remain dated 2026-06-10.

Extracted from §9 of
[ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md](ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md)
(the *base plan*) when that document grew too large. Cross-references of
the form **plan §N** point to base-plan sections; **Step N** /
**base-port Step N** refer to the steps of the plan §7 port plan; the
**vector doc** is
[vector-mode-analysis-port-plan-2026-06-10-en.md](vector-mode-analysis-port-plan-2026-06-10-en.md).
The consolidated landing order across all documents is the
[roadmap](ondemand-vec-fad-implementation-roadmap-2026-06-10-en.md)
(this document's phases A/B/C land as roadmap P4/P5/P8).

C++ reference: none for the combination — `fad`/`rad` are faust-rs
primitives with no C++ counterpart, so faust-rs defines the reference
semantics here and validation relies on numerical oracles (§8).

## 1. Goal

Analyze how the faust-rs differentiation primitives (`fad(expr, seed)`
forward mode, `rad(expr, seeds)` reverse mode) interact with clock
domains: what works today, what fails and *how* it fails, what the
correct mathematics is, and in what order to build the combination.

## 2. Why cohabitation matters (use cases)

The flagship use cases of FAD are **in-graph learning loops**
(`ad.lib`: `ad.grad`, `ad.fit_adam`, `ad.fit_rmsprop`, `ad.dense`,
`ad.newton`; corpus: `fad_filter3.dsp`, `fad_pendulum_cello4.dsp`, the
`auto_*` adaptive effects). Today every optimizer step runs **at audio
rate** — one Adam update per sample. The clock-domain primitives are
precisely the missing tool to make these loops cheap — and, in the other
direction, FAD is what makes the clocked blocks *adaptive*. Concretely:

**1. Control-rate in-graph learning.** Gradients may be cheap, but the
optimizer (`ad.fit_adam`: moments, clipping, clamping — per parameter) is
pure overhead at audio rate. Clock the update step every N samples; the
`PermVar` sample-and-hold output is *exactly* the right semantics for a
parameter held between updates:

```faust
upd = ba.pulse(64);                       // update clock, ~690 Hz at 44.1 kHz
p = tick ~ _ with {
  tick(prev) = (upd, prev) : ondemand(adam) with {
    adam(q) = ad.fit_adam(q, p_init, err, ad.grad(loss(q), q), lr, mn, mx);
  };
};
```

**2. Event-triggered adaptation.** With a *boolean* clock, `ondemand`
gates learning on a runtime condition. The canonical case: adapt only
while signal is present — today's `auto_*` effects keep optimizing on
noise during silence and drift away from the learned optimum:

```faust
active = an.amp_follower(0.05, x) > 0.001;   // freeze learning in silence
p = tick ~ _ with { tick(prev) = (active, prev) : ondemand(fit); };
```

The same shape covers feedback-killer notch tuning that runs only while
a larsen detector fires (`auto_notch_larsen.dsp`), envelope-triggered
re-calibration (`fad_calib1.dsp`), and one-shot system identification
("learn while the test signal plays, then hold forever").

**3. Decimated gradient estimation.** `downsampling(H)` around the
loss+gradient computation evaluates the gradient on every H-th sample —
a stochastic sub-sampled gradient, the in-graph analogue of minibatch
SGD. For block-based adaptive filters (`fad_fxlms*.dsp`, active noise
control) this matches how real LMS/FxLMS systems update. Bonus for
reverse mode: a decimated loss shrinks the `SigBlockReverseAD` tape and
the reverse-sweep cost by the same factor H (§7).

**4. Frame-rate DDSP controllers.** The neural helpers (`ad.dense`,
`ad.gru_cell`, `ad.lstm_cell`) run per-sample today, which is wildly
expensive and not even what DDSP architectures prescribe: control
networks run at *frame* rate (every 64–256 samples) and drive an
audio-rate synth through held/smoothed parameters. That is literally
`downsampling(controller)` + `PermVar` hold (+ `si.smooth` outside the
domain for de-zippering). `fad_pendulum_cello4.dsp` restructured this
way is the natural showcase.

**5. Runtime-count implicit solvers.** `upsampling(H)` executes its body
H times per outer tick with recursive state crossing the inner
iterations — i.e. it is an **in-graph iteration construct**. Newton
solvers with `fad`-computed Jacobians currently unroll a *static*
iteration count (`ad.newton(model, y0, K) = seq(i, K, newton_step)`).
Under `upsampling` the count becomes a *runtime signal*:

```faust
k = 1 + min(7, int(residual / tol));      // adaptive iteration count
y = (k, x) : upsampling(newton_step)      // newton_step uses fad(F, y)
```

— more iterations only when the implicit equation (diode clipper,
Duffing oscillator: `fad_diode_gemini.dsp`, `fad_duffing_gemini.dsp`) is
far from convergence. No static unrolling can express this.

**6. Lower oversampling factors via exact slopes (ADAA +
derivative-augmented decimation).** Two distinct mechanisms stack inside
`upsampling(H)`, and only the second one is FAD's contribution:

- *Classical ADAA* (antiderivative antialiasing, Parker et al. 2016)
  already trades oversampling for algebra — first-order ADAA plus ×2
  oversampling typically reaches the alias floor of plain ×8. But its
  ingredient is the **antiderivative** `F`, which FAD does *not*
  provide (FAD differentiates, it does not integrate): `F` must be
  supplied in closed form (`tanh` → `log(cosh)`, hard clip → piecewise
  quadratic).
- *Derivative-augmented decimation* is where `fad` earns its keep. By
  the generalized (Papoulis) sampling theorem, sampling value **plus
  first derivative** at rate R carries the information of value-only
  sampling at 2R. Computing both `y = f(x)` and `ẏ = f′(x)·ẋ` in the
  inner domain — `f′` exact via `fad(f(x), x)` (FAD skill pattern 2),
  `ẋ` from an inner-rate first difference or analytically for
  synthesized signals — lets a Hermite-reconstruction decimator target
  **H/2 instead of H** for the same alias floor.

Exact slopes additionally locate `f′` discontinuities for BLAMP-style
corner corrections (hard nonlinearities, whose ~1/k² harmonic rolloff
makes brute-force oversampling inefficient), and supply the Newton
Jacobians of case 5 for *implicit* nonlinearities (diode clipper), where
oversampling demands are worst. Honest caveats: none of this
*eliminates* aliasing — `f(x(t))` is not bandlimited, these techniques
lower the floor, and the gain tracks the harmonic rolloff of the
nonlinearity; the H/2 figure is a theoretical ceiling that assumes the
outer-domain decimator actually exploits the derivative channel (a
Hermite reconstruction filter after the bare `PermVar` hold, built in
the outer domain from the held `y` and `ẏ` lanes); and the `ma.SR`
adaptation inside US (plan §2.3) is what keeps time-constant-dependent
formulas correct at `SR·H`.

**7. Multi-timescale (nested) optimization.** Domains nest: an inner
`ondemand` adapts the DSP parameters every 64 samples, an outer (slower)
`ondemand` adapts *hyperparameters* — learning-rate decay on loss
plateau, gradient-clipping bounds — every 8192 samples. This is the
in-graph form of an optimization schedule, expressed as two nested clock
domains; today's host-driven pattern (FAD skill pattern 7) exists partly
because slow schedules have nowhere to live in the graph.

Note the staging payoff (§6): every pattern above except boundary-
crossing variants of 3 places `fad` **strictly inside one domain** —
Phase A. The killer use cases are reachable as soon as the base port
lands, before any cross-boundary AD work.

So the combination FAD × OD/US/DS is the natural completion of both
features, and the port plan should treat it as a roadmap item, not a
corner case.

## 3. Where the two features meet in the pipeline

Both are **propagation-stage** expansions over the same signal DAG:

- `ondemand/upsampling/downsampling` expand in `propagate_clocked_wrapper`
  ([engine.rs:840-915](RUST/faust-rs/crates/propagate/src/engine.rs)) into
  the boundary glue (`TempVar`/`double_clocked`/`ZeroPad` in,
  `PermVar`/`Clocked` out, `Seq(blockNode, permvar)` results).
- `fad` expands in the `FlatNodeKind::ForwardAD` arm
  ([engine.rs:648-677](RUST/faust-rs/crates/propagate/src/engine.rs)):
  seeds are propagated first, then the body, then
  `forward_ad::generate_fad_signals_multi` rewrites the body DAG into
  interleaved `[primal, tangent…]` lanes. `rad` follows the same wiring and
  dispatches to a symbolic feed-forward sweep or to the `SigBlockReverseAD`
  tape carrier ([reverse_ad.rs](RUST/faust-rs/crates/propagate/src/reverse_ad.rs)).

Consequently, whichever construct is syntactically *inner* is expanded
first:

- **`fad` inside an OD body**: the fad body and seeds are propagated under
  the inner clock env (`ctx.clock_env = clock_env2` is active across
  `propagate_in_slot_env(body, …)`). The tangent lanes built by
  `forward_ad` are ordinary arithmetic over inner-domain signals; the
  bottom-up clock inference of plan §3.6 will place them in the inner domain
  automatically. **No new inference rule is needed** — a direct benefit of
  the "env of a signal is determined only by its inputs" property (plan §4.1).
- **`fad` around an OD wrapper**: by the time `generate_fad_signals_multi`
  runs, the body signals already contain
  `Seq(OD, PermVar(Clocked(c, y)))` shapes, so the forward-AD transform
  meets the glue nodes head-on. This is where the current implementation
  is wrong (next section).

A seed is matched by **node identity** (hash-consed `SigId`). A slider
used both outside and inside a domain is the *same* node (UI elements are
not slot-bound, so they receive no `Clocked` wrapper), so seed matching
works across the boundary as-is. The C++ `recTempVar` stacking for
slot-bound signals (plan §3.2) is the one mechanism that may *rename* a signal
between domains; the FAD seed-identity contract must be re-checked when
that part is ported.

## 4. Current behavior (validated experimentally, 2026-06-10, `main-dev`)

| Program | Result today |
|---|---|
| `ondemand` alone | `FRS-SFIR-0004` (the plan §6.3 `signal_prepare` bug — clock env traversed as a signal) |
| `fad( … : ondemand(*(g)), g)` | propagation **succeeds silently** (zero tangents), then dies at the same unrelated `FRS-SFIR-0004` |
| `ondemand(fad(*(g), g))` | same: silent propagation, then `FRS-SFIR-0004` |
| `rad( … : ondemand(*(g)), g)` | `FRS-PROP-0001 — rad cannot differentiate signal node (other)` — loud and correct, but the message does not name `ondemand` |

The mechanism behind the silent `fad` cases is the catch-all in
[forward_ad.rs:1075](RUST/faust-rs/crates/propagate/src/forward_ad.rs):
`Seq`, `Clocked`, `TempVar`, `PermVar`, `ZeroPad`, `OnDemand`,
`Upsampling`, `Downsampling` all fall through to `zero_tangent(sig)` —
primal preserved, **every tangent lane silently zero, children not
traversed**. RAD instead rejects loudly by design
([reverse_ad.rs:322-334](RUST/faust-rs/crates/propagate/src/reverse_ad.rs));
note that `stateful_rad`'s classifier already treats the glue
conservatively and correctly skips the clock-env child of `Clocked`
([stateful_rad.rs:625,818](RUST/faust-rs/crates/propagate/src/stateful_rad.rs)).

**The correctness cliff.** Today the unrelated `FRS-SFIR-0004` failure
masks the zero-tangent bug. The moment base-port Step 1 fixes
`signal_prepare`, `fad`-across-a-boundary will *compile and run with
silently wrong (zero) gradients* — the worst failure mode for a learning
loop, which will simply not converge. Therefore: **a FAD diagnostic on
boundary glue must land in the same step as the `signal_prepare` fix**
(see §8).

There is no upstream reference for this combination: `fad`/`rad` are
faust-rs primitives with no C++ counterpart, so faust-rs gets to define
the reference semantics, and validation must rely on numerical oracles
(§8) rather than differential testing.

## 5. The mathematics: differentiation commutes with the boundary

Fix a parameter (seed) θ and assume **the clock H does not depend on θ**.
Then every boundary operator is *linear time-varying with
θ-independent timing*, and differentiation commutes with all of them:

| Boundary operator | Forward sample semantics | Tangent rule |
|---|---|---|
| snapshot (`TempVar` + `double_clocked`) | read `u[n]` at the fire instant | `(snap u)' = snap(u')` |
| hold (`PermVar`, init 0) | `y[n] = u[τ(n)]`, `τ(n)` = last fire ≤ n | `(hold u)' = hold(u')` — S&H commutes with d/dθ |
| zero-pad (`ZeroPad(u, H)`, US input) | `u` on last inner iteration, else 0 | `(pad u)' = pad(u')`, `H` not differentiated |
| per-domain time (local `IOTA`) | inner delays advance only on fire | tangent state interleaved in the *same* rec groups fires on the same clock — primal/tangent automatically co-clocked |
| init | `PermVar` starts at 0 | consistent: before the first fire, primal = 0 *and* tangent = 0 |

The interleaved augmented-state recursion model that FAD already uses
(one carrier `[primal, d/ds0, …]` per rec group) pays off here: because
tangent state lives in the same recursive group as primal state, both
advance under the same clock with the same local `IOTA` *by
construction* — there is no way for them to de-synchronize.

**If the clock does depend on θ** (e.g. a comparator on a learned
signal): the firing *times* move with θ and the exact derivative gains
Dirac-like terms at firing boundaries. Policy: ignore them — the clock is
non-differentiable discrete control, exactly like the existing `select2`
selector and `int_cast` rules (zero tangent through the condition). This
is a documented approximation boundary, not a bug.

Conclusion: the exact forward rule is **purely structural** —
differentiate the body and duplicate the boundary machinery for the tangent
lanes under the *same clock env as the augmented primal*. That env must be
distinct from the original block's if the original remains reachable (§6):

```
fad(ondemand(C), s)  ≡  ondemand(C_aug)     C_aug emits [primal lanes, tangent lanes]
```

and identically for `upsampling`/`downsampling`.

## 6. FAD design: block augmentation ("augment once")

Concrete `transform()` rules for `forward_ad.rs` (Phase B below):

| Node | Dual rule |
|---|---|
| `TempVar(u)` | primal `TempVar(u_aug)`; tangent `TempVar(u')` |
| `Clocked(c, u)` | primal `Clocked(c_aug, u_aug)`; tangent `Clocked(c_aug, u')` — rename a twinned domain's opaque token; do not differentiate it |
| `double_clocked(c2, c1, u)` | same boundary structure over transformed primal/tangents, mapping each twinned domain token |
| `ZeroPad(u, H)` | tangent `ZeroPad(u', H_aug)` with the transformed primal factor, without differentiating the firing schedule; current variable-factor reconstruction gap recorded in the 2026-09-19 review |
| `PermVar(u)` | `PermVar(u')` |
| `Seq(OD, y)` | primal `Seq(OD_aug, y_aug)`; tangent `Seq(OD_aug, y')`, except literal zero tangents need no sequencing |
| `OD/US/DS(clockedClock, Y…)` | `OD_aug` = same kind, transformed primal clock, transformed primal/tangent held lanes in a fresh domain — built **once per source block node per FAD transform** (memoized) |

Design constraints discovered by this analysis:

1. **One augmented block for its primal and tangents.** Rewritten
   `Seq(OD, …)` consumers point at the memoized `OD_aug`; they must not
   execute one body per tangent. An independent consumer outside `fad`
   may still read the original `OD`, so the original is not necessarily
   unreachable. Both blocks can execute, provided they own separate state.
2. **The augmented block owns a fresh domain.** Since `9e5d9320`,
   `OD_aug` receives a new `ClockDomain` identity and its nested domains
   are twinned with mapped parents. Since `21252f16`, rebuilding covers
   seed-independent subtrees and zero-tangent fallback rules too: no token
   of an original domain being twinned, or its descendants, may survive
   in the augmented payload. Its clock is rebuilt as a primal signal even
   though timing decisions are not differentiated. Reusing the original's
   domain, as this plan initially proposed, shared delays between two
   executable blocks when the original was also consumed.
3. **Tangent lanes terminate at the boundary by recursion into the outer
   domain**: `(snap u)' = snap(u')` re-enters `transform(u)` in the outer
   DAG, so a chain `fad(g*_ : ondemand(F), g)` correctly picks up the
   `∂(g·x)/∂g` contribution that today is silently dropped.
4. **`suppress_fad`/`RecFadMode::ExpandAfterRec` interplay** (the deferred
   FAD expansion used when `fad` sits under a `Rec` branch,
   [engine.rs:551-621](RUST/faust-rs/crates/propagate/src/engine.rs)): if a
   clocked wrapper sits between the `Rec` and the suppressed `fad`,
   `pending_fad_seeds` cross a domain boundary. The deferred expansion
   then runs on outputs that contain glue nodes — Phase B rules cover it,
   but this nesting needs a dedicated test (and a diagnostic until then).

**Staging.** Phase A — `fad` strictly inside one domain (seed = slider or
domain-local signal, differentiated path never crosses the boundary) —
needs **zero new FAD code**: the tangent DAG is domain-local arithmetic
and the base-port Steps 1–6 carry it like any other inner signal. It
needs tests, plus the boundary diagnostic so that accidental crossings
fail loudly instead of converging to nothing. Phase B is the table above.

## 7. RAD: the transpose of a clock domain

Reverse mode is harder for a structural reason: **the adjoints of the
boundary operators are their transposes**, and transposition swaps the
boundary roles (classic multirate identities):

| Forward operator | Transpose (adjoint propagation) |
|---|---|
| hold (`PermVar`) `y[n] = u[τ(n)]` | `ū[k] = Σ_{n: τ(n)=k} ȳ[n]` — **accumulate over the hold period, deposit at the fire instant** |
| zero-pad (US input) | sample at the last inner iteration (decimation) |
| snapshot (sampling at fires) | impulse placement: adjoint deposits only at fire instants |
| upsampling block `US(B)` | downsampling-with-accumulation of `Bᵀ` |
| downsampling block `DS(B)` | upsampling-with-zero-stuffing of `Bᵀ` |
| `ondemand(B)` | `ondemand(Bᵀ)` on the same firing pattern with hold↔accumulate swapped at its boundaries |

Within the existing `SigBlockReverseAD` tape architecture the picture is
clean: running the block tape *backwards*, the adjoint of `ondemand` is a
**gated integrate-and-dump** — accumulate `ȳ` while scanning the hold
period in reverse, and on reaching a recorded fire instant, push the
accumulator through the transposed body and deposit into `ū`. The costs:

- the tape must become **clock-aware**: record the evaluated clock per
  outer tick (firing pattern) and use variable-rate storage for inner
  intermediates (×H entries per outer tick under US, ÷H under DS);
- per-domain reverse time: the backward sweep must respect each domain's
  local time, i.e. the tape needs per-domain time stamps.

The YOLO linearize-once/transpose path
(`porting/yolo-linearize-once-rad-analysis-2026-05-21-en.md`) gets the
*constant-rate* cases almost for free: with frozen controls and constant
integer `H`, US/DS blocks are linear **periodically**-time-varying, and
the transpose is the static block swap of the table above. `ondemand`
with a data-dependent boolean clock is genuinely LTV with a
data-dependent schedule — only the tape route covers it.

Recommendation: keep the loud rejection (already the current behavior —
the right call), but **improve the message** to name the construct
(today: kind `"other"`; it should say `ondemand/upsampling/downsampling
inside rad is not supported yet`). Implement the clock-aware tape only
after base-port Step 6 and FAD Phase B have proven the forward
semantics; the LPTV transpose for constant-rate US/DS can come last as an
optimization.

## 8. Sequencing and test corpus

Order of work (interleaved with the base plan (plan §7)):

1. **With Step 1** (same change set): FAD diagnostic on boundary glue —
   replace the silent `zero_tangent` fallback for
   `Seq/Clocked/TempVar/PermVar/ZeroPad/OD/US/DS` with a structured
   `FRS-PROP` error ("fad cannot yet differentiate across an
   ondemand/upsampling/downsampling boundary"). Removes the correctness
   cliff of §4. Also improve the RAD `"other"` message (one-liner).
2. **After base Steps 1–6**: FAD Phase A — corpus + runtime tests for
   `fad` strictly inside a domain (control-rate `ad.fit_adam` under
   `ondemand`, oversampled slope under `upsampling`).
3. **FAD Phase B**: block augmentation per §6; relax the diagnostic.
4. **RAD Phase C**: clock-aware tape; then optionally the LPTV transpose.

Corpus additions (note: **no differential testing against C++ is possible
here** — upstream has no `fad`; the oracle is the finite-difference
harness already used by `fad_recursive_runtime.rs` / `rad_runtime.rs` /
`block_reverse_ad.rs`):

- `fad` inside `ondemand`, slider seed (Phase A happy path);
- control-rate Adam: `ondemand(fit_adam_step)` with a pulse clock,
  convergence test vs audio-rate reference (§2 case 1);
- event-gated learning: boolean clock from a level detector, asserting
  parameters freeze exactly during silence (§2 case 2);
- runtime-count Newton under `upsampling` with a signal-valued iteration
  count, vs the statically unrolled `ad.newton` reference (§2 case 5);
- `fad` inside `upsampling` (exact slope), tangent vs finite differences
  at the oversampled rate, plus an alias-floor measurement comparing
  value-only decimation at H against derivative-augmented (Hermite)
  decimation at H/2 (§2 case 6);
- `fad` around `ondemand` with the seed feeding the wrapper *input*
  (boundary crossing: diagnostic in step 1, exact value in Phase B);
- seed = the clock itself, and clock-depends-on-seed (documented
  zero/approximation, must not crash);
- `ondemand` nested in `ondemand` under one `fad` (env reuse + block
  augmentation at two depths);
- `fad` under `Rec` with a clocked wrapper in between
  (`suppress_fad` interplay, §6 point 4);
- `rad` around each wrapper kind (diagnostic snapshot until Phase C).
