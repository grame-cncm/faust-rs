# FAD Phase B (P5) — status and the road to S4 (differentiable STFT)

**Date**: 2026-07-09
**Branch**: `ondemand-vec-fad-synthesis`
**Scope**: `crates/propagate/src/forward_ad.rs` (the boundary dual rules), the
S4 spectral-loss milestone.
**Related**: [`ondemand-fad-rad-cohabitation-2026-06-10-en.md`](ondemand-fad-rad-cohabitation-2026-06-10-en.md)
§5 (the maths) / §6 (the dual-rule table), [`ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md`](ondemand-vec-fad-interleave-synthesis-2026-07-07-en.md)
§7 (S1–S5), `docs/ondemand-fft-spectral-comparison-en.md`.

---

## 1. One-paragraph verdict

S4 (a **differentiable STFT** / in-graph spectral loss for DDSP) is **one
well-scoped step away**, and that step is **P5 = FAD Phase B (boundary
crossing)**. P5 is not open research: the mathematics is proven, the dual rules
are tabulated, the blocker is a single match arm, and the STFT infrastructure is
already built and compiles at O(N log N). What remains is a mostly-structural
rewrite plus a battery of finite-difference tests.

## 2. What is already in place (the S4 infrastructure exists)

- **FAD Phase A (P4) — done and tested.** `fad` strictly *inside* a clock domain
  works and is validated against central differences. It needed **zero new AD
  code**: the tangent DAG is domain-local arithmetic the back-half carries like
  any inner signal. Anchor test: `crates/compiler/tests/ondemand_pipeline.rs`
  (the FAD sweep that "never touches boundary glue").
- **The correctness cliff is guarded.** `fad` crossing a boundary raises a loud
  `FRS-PROP-0004` (never silently-zero tangents that would compile a learning
  loop which never converges). Tested in `ondemand_pipeline.rs` (the boundary
  rejection cases).
- **The STFT infrastructure is ready:** the S3 analysis-only framed FFT, the
  phase-vocoder inter-frame machinery (2026-07-09), and **Phase A per-scope CSE**
  which keeps the framed FFT O(N log N) (commit `03127f30`).

## 2b. Progress — the wrapper half of P5 is done (2026-07-09)

The **boundary wrapper rules** are implemented in `forward_ad.rs`: the single
error arm was split so `TempVar`/`PermVar`/`Clocked`/`ZeroPad` now differentiate
via `(wrapper u)' = wrapper(u')` (value child carries the tangent; the
clock/clock-env child is opaque). Verified:

- unit tests on the four wrappers (`fad_phase_b_wrapper_boundaries_pass_through`);
- **numeric parity end-to-end**: `ondemand(fad(*(g), g))` — previously rejected
  with `FRS-PROP-0004` — now differentiates correctly through the block's
  `TempVar` data input: tangent = held input snapshot, `primal == g · tangent`
  (`fad_inside_ondemand_crossing_seed_reads_clocked_input`, run through the
  interpreter). 76/76 workspace test binaries + 190 goldens unaffected.

Still open (§4): the **block-augmentation** half — `Seq(OD, y)` and
`OD/US/DS → OD_aug` — which keeps erroring loudly until it lands.

### 2c. S4 core reached via the "fad inside the block" form

The wrapper rules already unlock a **differentiable STFT spectral loss** — the S4
milestone — when the loss is computed *inside* the frame block and the learnable
parameter scales the window inside it. `ondemand(fad(loss(g·window), g))`: the
seed path crosses the block's `TempVar` window inputs (wrapper rules) and threads
through the FFT + magnitude + nonlinear loss (existing fad rules).

Demonstrated by `tests/corpus/ondemand_fad_spectral_loss_008.dsp` (framed FFT via
`interleave.lib`): the block emits `[loss, dloss/dg]` at frame rate. Analytically
exact — an impulse window gives a flat spectrum, `Σ|g·X| = 8g`, so
`loss = (8g − 4)²`, `dloss/dg = 2(8g − 4)·8`; at g=1 the block outputs
`[16, 64]`, matching. Gradient also validated vs central differences on an
inline nonlinear frame reduction
(`fad_inside_ondemand_nonlinear_frame_reduction_matches_central_difference`).

### 2d. P5 complete — block augmentation (`OD_aug`) landed (2026-07-09)

The **fad-*outside*** form now works too: `fad(EXPR : ondemand(F), θ)`
differentiates through the block from outside. `forward_ad.rs` gained
`transform_seq` + `augment_block`: `Seq(OD, y)` → `{ Seq(OD_aug, y),
Seq(OD_aug, y') }`, where `OD_aug` rebuilds the wrapper payload as
`[clock, lane₀, lane₀', lane₁, lane₁', …]` (primal + one tangent per seed per held
lane), **memoized once per source block** (`od_aug_cache`) so every `Seq` consumer
shares one augmented block (§6.1 "one block, not two"). The clock lane is opaque.
A bare wrapper reached outside a `Seq` stays a loud error (defensive).

Verified numerically through the interpreter:
- `fad_around_ondemand_differentiates_through_the_block`:
  `fad((clk,x):ondemand(*(g)), g)` → tangent = held input, `primal = g·tangent`;
- `fad_around_ondemand_nonlinear_body_gradient_is_exact`:
  `fad((clk,x):ondemand((g·x)²), g)` → tangent = `2·g·x²` exactly;
- CLI: nonlinear `(g·x)²` matched central differences (9.0000).

So **both forms of P5 are done**: fad-inside (§2c) and fad-outside (§2d). This
unblocks the full S4 surface, including time-domain reconstruction spectral
losses (loss read after `serialize_out`). 76/76 workspace binaries, 190 goldens
unaffected, clippy clean.

## 3. The exact blocker — one match arm (before 2b)

In `crates/propagate/src/forward_ad.rs` (the `transform()` dispatch), all **eight**
boundary node kinds — `Seq`, `Clocked`, `TempVar`, `PermVar`, `ZeroPad`,
`OnDemand`, `Upsampling`, `Downsampling` — fall into a **single** arm that sets
`self.boundary_error` and returns `zero_tangent(sig)`. That arm *is* the P0.4
diagnostic. `error.rs` states it verbatim: *"roadmap P5 relaxes this by
implementing the boundary dual rules."*

## 4. What P5 is — the dual rules (proven, mostly structural)

Cohabitation §5 proves differentiation **commutes with every boundary operator**
as long as the clock does not depend on the seed θ (a comparator on a *learned*
signal moves the firing times → Dirac terms; policy is to ignore them, exactly
like `select2`/`int_cast` — a documented approximation boundary, not a bug).

P5 replaces the error arm with the §6 table:

| Node | Dual rule |
|---|---|
| `TempVar(u)` | `TempVar(u')` |
| `PermVar(u)` | `PermVar(u')` |
| `Clocked(c, u)` | `Clocked(c, u')` — the clock-env child is opaque, never traversed |
| `ZeroPad(u, H)` | `ZeroPad(u', H)` — `H` not differentiated |
| `Seq(OD, y)` | `{ primal: Seq(OD_aug, y), tangent: Seq(OD_aug, y') }` |
| `OD/US/DS(clock, Y…)` | `OD_aug` = same kind, same clocked clock, payload `Y ∪ Y'`, built **once per source block node** (memoized) |

The non-trivial constraint is **"one block, not two"** (§6.1): memoize the
`OD → OD_aug` rewrite per source `SigId` and route *every* `Seq` consumer through
it. If the original block stayed reachable next to the augmented one, a stateful
body (delays, the FFT recursion) would execute twice per fire and its local
`IOTA`/state would advance twice. The clock-env identity is reused (legal, §6.2:
block augmentation rewrites *signal* nodes only, never re-mints a domain). Tangent
lanes terminate by recursion into the outer domain (§6.3), so a chain
`fad(g*_ : ondemand(F), g)` picks up the `∂(g·x)/∂g` contribution that is
silently dropped today.

## 5. What S4 needs on top of P5 (little)

A spectral loss differentiates w.r.t. a parameter θ that enters the block through
`serialize_in` → the FFT runs on a θ-dependent window → the gradient crosses the
`ondemand` boundary. So **S4 ≈ P5 + details**:

1. **Seed matching: nothing to do.** Parameters (`hslider`) are not slot-bound, so
   they receive no `Clocked` wrapper; node-identity seed matching works across the
   boundary as-is (cohabitation §4).
2. **Magnitude epsilon.** `|X| = sqrt(R² + I² + ε)`; the fad `sqrt` rule already
   exists (`forward_ad.rs`), but the derivative blows up at `R=I=0`, so ε is the
   DSP author's responsibility (a library detail, not a compiler one).
3. **A dedicated test** for the `suppress_fad` / `RecFadMode::ExpandAfterRec`
   interplay when a clocked wrapper sits between a `Rec` and a suppressed `fad`
   (§6.4).

## 6. Interaction with the runtime work (this session)

**Phase A (per-scope CSE) is a practical prerequisite for S4.** The augmented
block `OD_aug` carries the payload `Y ∪ Y'` — it **doubles the lanes**
(primal + tangent). Without per-scope CSE the augmented FFT would be O(N²·⁶) and
effectively uncompilable; with it, the differentiable FFT stays O(N log N). The
OLA runtime cost (Phase C of the scalability plan) is orthogonal — it affects
speed, not gradient correctness.

## 7. Scope, oracle, and first step

- **Scope**: localized to `forward_ad.rs` — ~7 dual-rule arms replacing 1 error
  arm — plus the "augment once" memoization. No new signal node, no backend
  change (the augmented graph reuses the existing clocked lowering).
- **Oracle**: **finite differences** (there is no C++ reference for FAD × clock
  domains — faust-rs defines the semantics).
- **First step (toy before STFT)**: `fad(g*_ : ondemand(F), g)` — its
  `∂(g·x)/∂g` is silently lost today. Make it match central differences, then
  scale up to a magnitude spectral loss over the framed FFT.

## 8. Strategic payoff

P5 unlocks **spectral DDSP**: a trainable spectral loss *inside* the Faust graph
(gradient through the FFT), which no mainstream real-time environment offers.
That is the differentiator this whole workstream has been building toward — the
S3 framed FFT and the phase-vocoder machinery are its infrastructure, and P5 is
the last correctness gate.
