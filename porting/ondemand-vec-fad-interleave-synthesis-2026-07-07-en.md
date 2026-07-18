# Synthesis: Clock Domains (ondemand/US/DS), Vector Mode, FAD/RAD, and the `interleave` Primitive for Spectral Computation

Date: 2026-07-07

Status: proposed — supersedes the
[2026-06-10 roadmap](ondemand-vec-fad-implementation-roadmap-2026-06-10-en.md)
as the tracking surface; the three analysis documents remain the
per-topic technical references.

French version: [ondemand-vec-fad-interleave-synthesis-2026-07-07-fr.md](ondemand-vec-fad-interleave-synthesis-2026-07-07-fr.md)
(same content; keep both in sync on amendment).

Document family (cross-reference conventions unchanged):

- **plan §N**: [ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md](ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md) — C++ clock-domain analysis + the 8-step port plan;
- **cohabitation §N**: [ondemand-fad-rad-cohabitation-2026-06-10-en.md](ondemand-fad-rad-cohabitation-2026-06-10-en.md) — FAD/RAD × clock domains (phases A/B/C);
- **vector doc §N**: [vector-mode-analysis-port-plan-2026-06-10-en.md](vector-mode-analysis-port-plan-2026-06-10-en.md) — `-vec` port (V1–V6) and composition with domains (D1/D2);
- **roadmap PN**: the consolidated 2026-06-10 roadmap (phases P0–P9), whose
  landing order this document takes over and updates.

New relative to 2026-06-10:

1. **faust-rs state re-verified on 2026-07-07** (§2): nothing of P0–P9 has
   landed, but four intermediate refactors moved the target files and
   lowered the cost of several phases.
2. **The spectral track** (§4–§5): analysis of the `interleave` primitive
   (time ↔ frame-width serialization around `ondemand`) which makes
   *frame-rate processing* (STFT, per-frame FFT, differentiable spectral
   loss) expressible in pure Faust — and its insertion into the plan
   (track S1–S5, §7).
3. **A cross-workstream interaction**: the in-flight `propagate`
   memoization port (2026-07-04 plan) must include the clock environment
   in its cache key, or it reintroduces the P0.3 bug (§2.3).

C++ reference unchanged: branch `master-dev-ocpp-od-fir-2-FIR19`, commit
`8eebea429` for the clocked machinery; upstream `master` for base `-vec`;
**no C++ reference** for FAD/RAD × domains or for `interleave` — faust-rs
defines the semantics, the oracle is numerical.

## 1. The stakes on one page

Four workstreams, one semantic core:

1. **Clock domains** (`ondemand`/`upsampling`/`downsampling`). The front
   half (syntax → boxes → eval → propagation → typed clocked signal
   graph) is **already at parity** in faust-rs (plan §6.1). The back half
   does not exist: clock-environment inference, hierarchical dependency
   graph, guarded blocks in the FIR, per-domain local time
   (`IOTA`/`DSCounter`), backend emission. That is the port proper
   (plan §7, roadmap P0–P3).
2. **FAD/RAD × domains** — the applicative motivation: control-rate
   in-graph learning (`ondemand(ad.fit_adam …)`), event-triggered
   adaptation, decimated gradients, frame-rate DDSP controllers
   (cohabitation §2). Two facts structure everything:
   - **The correctness cliff**: today `fad` across a boundary produces
     *silently zero* tangents (cohabitation §4), masked by the unrelated
     downstream `FRS-SFIR-0004` failure. The day `signal_prepare` accepts
     clocked nodes, a learning loop will compile and simply never
     converge. The loud FAD diagnostic must land **in the same change
     set** as the `signal_prepare` fix (P0 is indivisible).
   - Differentiation **commutes with every boundary operator** as long
     as the clock does not depend on the seed (cohabitation §5): `fad`
     strictly *inside* a domain needs **zero new AD code** (Phase A);
     exact boundary crossing is a structural "augment once" rewrite
     (Phase B); RAD requires a clock-aware tape (Phase C), except in
     constant-rate cases where the LPTV transpose of the YOLO path
     suffices.
3. **Vector mode** (`-vec`): absent from faust-rs, disabled outright on
   the C++ research branch. faust-rs ports it **once** (single lowering
   site `signal_fir`, vector doc §4) as a deterministic `LoopGraph`
   (V1–V6), then composes it with domains via **scalar islands** (D1):
   each OD/US/DS block becomes a serial loop node whose interface is
   exactly the `TempVar`/`PermVar` glue — bit-exact, no option rejection.
   The shared CSE invariant ("never hoist across a region boundary") is
   built once (P2) for both consumers.
4. **Spectral computation** (`interleave`, new): Faust can only do FFT in
   the *sliding* regime (per-sample recomputation, `analyzers.lib`). The
   *frame-rate* regime — STFT, phase vocoder, DDSP spectral losses — is
   not expressible. The analysis (§4) shows `ondemand` already provides
   almost everything: **one single brick** is missing, the zero-stuffed
   output (`↑₀`, dual of decimation), and the spatial FFT core
   (`an.fftb`) is reused as-is. Decisive bonus: a pure-Faust STFT is
   automatically differentiable — this is the infrastructure for
   in-graph spectral losses.

## 2. faust-rs state as of 2026-07-07 (verified on `main-dev`)

### 2.1 Nothing of roadmap P0–P9 has landed

Every point re-verified in the sources:

| Finding | Evidence | Affected phase |
|---|---|---|
| The clock environment is still traversed as a signal: `Clocked(x, y)` shares a match arm with `Seq`/`ZeroPad` and visits `x` | `crates/transform/src/signal_prepare/verify.rs:257` | P0.1 |
| `make_clock_env` still leaves `slotenv`/`path` nil (instance-uniqueness bug) | `crates/propagate/src/engine.rs:1086` | P0.2 |
| The silent `zero_tangent` fallback still catches all the glue (`Seq`/`Clocked`/`TempVar`/`PermVar`/`ZeroPad`/OD/US/DS) via `_ =>` | `crates/propagate/src/forward_ad.rs:1075` | P0.4 |
| The RAD rejection still says `kind: "other"` without naming the construct | `crates/propagate/src/reverse_ad.rs:331` | P0.4 |
| No clock-inference module (`clk_env` absent from `crates/transform/src/`) | tree listing | P1 |
| No `ComputeMode` in `SignalFirOptions`, no `-vec`/`-vs`/`-lv` plumbing | `crates/transform/src/signal_fir/mod.rs:119` | P6 (V1) |
| No trace of `interleave` (parser, boxes, signals) | grep | track S |

### 2.2 What changed since 2026-06-10: four refactors that move the targets

None implements a phase, but all change the files the 2026-06-10 plan
cited, and two genuinely lower the cost:

1. **`signal_prepare` restructured** (`9841595b`, `e18d20f9`, `7b8d118e`):
   now `crates/transform/src/signal_prepare/{mod,verify,rewrites}.rs`
   with a typed `Staging` driver. The P0.1 fix now targets `verify.rs`
   (pull `Clocked` out of the shared arm at line 257 and stop visiting
   the first child) and `rewrites.rs` for the canonicalizations.
2. **`delay.rs` split up** (`aa94c747` → `6558a85e`):
   `signal_fir/delay/{manager,plan,context}` + per-strategy emission via
   `DelayKind`, unified `plan_delays` walk. The "per-domain IOTA"
   integration points (P2.3/P3.1) and the "vector delay strategies" (V4)
   land on this per-strategy structure — considerably more welcoming
   than the monolith described in vector doc §4.
3. **`SignalToFirLower` decomposed** (W9, `df1786a3` → `6366cbab`):
   extracted sub-structs (`ModuleSections`, `PlacementInfo`,
   `UiLoweringState`, `NameGen`, `RadReverseState`, `BraState`…). The P2.2
   region refactor — replacing the flat `sample_phases` accumulator with
   a region tree — starts from an already-decomposed state; the "big
   refactor on a big struct" risk dropped a notch.
4. **Shared C-family emitter core** (`5c6db8d7` + `068fbaa6`, all 7
   drifts closed): structured emission of guarded blocks (P3.2) and of
   chunk drivers (V5) is written **once** for c/cpp instead of twice.

Consequence: the roadmap size estimates hold except P2 (M–L → rather M)
and P3.2/V5 (emission once instead of twice).

### 2.3 Interaction with the in-flight `propagate` memoization work

The plan
[cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md](cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md)
will introduce a result memo in `propagate_in_slot_env` — the main gap it
identified. **Cross-constraint**: in C++ the `propagate` memoization key
includes the `clockenv` (plan §3.2, [propagate.cpp:918-929]). If the Rust
memo lands with a key that omits the clock environment, the same box
propagated under two domains will return the same signal — exactly the
bug P0.3 was meant to audit, except it would be *created* by the perf
port instead of inherited. To be recorded as a requirement of the
memoization plan now, even though domains are not yet usable downstream:
propagation itself already builds clocked graphs.

## 3. Condensed recap of the foundation (pointers)

To avoid duplicating the analyses, only the operational essentials:

- **Phase architecture** (plan §3–§5): propagation marks the boundaries
  (`TempVar`/`double_clocked` in, `PermVar`+`Clocked` out,
  `Seq(OD, permvar)` as ordering constraint) → inference assigns each
  signal to its domain (monotone clock calculus, Kleene fixed point,
  strictly nested domains) → the hierarchical `Hgraph` partitions →
  scheduling produces `Hsched` → code generation materializes guarded
  blocks, local times, and boundary variables. Verdict of plan §5.3:
  **port the algorithms 1:1, modernize the representations**
  (`ClockDomain` arena with a uniqueness token instead of the cons
  tuple; side map instead of tree property; single deterministic
  toposort; structured `FRS-` diagnostics).
- **FAD/RAD** (cohabitation §5–§7): tangents live in the same recursive
  groups as primals, so primal/tangent are co-clocked *by construction*;
  the crossing rule is "augment the block once" (a single execution of
  the body per fire tick, same clock env reused); the adjoint of
  `ondemand` is a gated integrate-and-dump (hold ↔ accumulate-at-fire,
  zero-pad ↔ decimate), hence a clock-aware tape for the general case
  and an LPTV transpose for constant rates.
- **Vector mode** (vector doc §2, §5–§6): deterministic `LoopGraph`
  (`LoopId` arena, `needSeparateLoop` criterion ported verbatim, chunk
  buffers, two block-level delay layouts, `-lv 0|1` drivers), scalar
  islands for clocked blocks (D1), vectorization of literal-factor US/DS
  interiors (D2). RAD policy: modules with a reverse-time loop force
  scalar mode until the TBPTT-window-under-chunking question is settled.

## 4. The spectral track: analysis of the `interleave` primitive

Source: the analysis conversation `RUST/INTERLEAVED.md` (derivation of
`gather`/`scatter` in terms of `↓`/`↑`/`@`/`od`), formalized and
corrected here. No C++ reference: this is a faust-rs extension (and a
proposal that can be upstreamed).

### 4.1 The need and the gap

`analyzers.lib` can already do an FFT — but in the **sliding** regime:
`an.fft(N) = si.cbus(N) : an.c_bit_reverse_shuffle(N) : an.fftb(N)` is a
purely spatial circuit (no `@`, `~`, or `mem` inside `fftb`; twiddles =
compile-time constants) recomputed **every sample** — O(N log N) *per
sample*. The standard frame-rate regime (one transform per hop,
O(N log N) *per frame*) is not expressible: the clocking is missing.
`ondemand` is exactly that clocking; what is missing is the
**time ↔ frame-width** conversion.

### 4.2 The decomposition: three stages, only one missing

```
interleave(N, FX) = serialize_in(N) : periodic_od(FX) : serialize_out(N)
```

with a periodic clock `H` of period `N` and **phase N−1** (tick at
instants `t ≡ N−1 (mod N)`, i.e. right when the window has just been
filled):

1. **`serialize_in(N) = par(i, N, _@(N-1-i))`** — **pure sugar** over
   the existing delays. At tick `t = kN−1`, line `i` carries
   `x((k−1)N + i)`: the contiguous window, in order. Crucial design
   point ("montage (b)" of the analysis): `serialize_in` operates
   **outside** the `od`, at full rate — inside the decimated domain,
   `@1` would mean one tick = N samples and the window would no longer
   be contiguous.
2. **`od(FX)`** — reused as-is. Decimating the N lines by `↓H` compacts
   the ticks: in decimated time, `FX` sees one true frame per step. This
   is the base port (P3) with nothing added.
3. **`serialize_out(N)`** — the missing brick. The output of `ondemand`
   is a **hold** (`PermVar`, sample-and-hold): during a whole block of N
   samples, the N lines carry the N frame values *in parallel*. To
   reconstruct a temporal stream you need a **temporal demultiplex**:
   line `j` must be non-zero only in its slot. The `↑`-hold does not
   provide it; the missing dual is zero-stuff upsampling `↑₀` — which is
   also the **true transpose** of decimation in the inner-product sense
   (the hold is not), making it the right brick for gradients (§4.5).

Vocabulary caution: the existing node `SigZeroPad(x, H)`
(`crates/signals/src/lib.rs:1191`) is the **input** glue of `upsampling`
(value on the last inner iteration, 0 otherwise) — related to but
distinct from the **output** `↑₀` discussed here.

### 4.3 Three options for `↑₀`, and the recommendation

**Option A — library sugar (recommended for V1).** For a *boolean
periodic* clock, the fire indicator is the clock itself, readable at the
outer rate. And the `Seq(OD, PermVar(…))` output holds the fresh value
from the fire tick onward (the `Seq` guarantees block-before-read within
the same tick). Hence:

```faust
up0(H, y) = y * (H != 0);          // zero-stuff = hold masked by the clock
serialize_out(N) = par(j, N, up0(H) : @(j)) :> _;
```

Zero new primitive, zero new signal node: `interleave` becomes a
**library definition** on top of the base port. Accepted restriction:
boolean clock (not integer OD or US, where "fire" ≠ value of H at the
outer rate) — which covers the entire STFT case.

**Option B — dedicated signal node** (`SigZeroVar`, dual of
`SigPermVar`: value at fire, 0 otherwise). Cleaner semantics, native
exact transpose for RAD (P8: the adjoint of `↑₀` is decimation, without
going through the product rule + non-differentiable clock), cost: one
node through the whole pipeline (typing, prepare, inference, lowering,
backends). Only do it if option A shows a real gradient or performance
problem.

**Option C — monolithic `interleave` primitive**: rejected; it would
duplicate machinery `ondemand` already provides.

### 4.4 Latency and phase convention — derivation, to be locked by the N=2 table

With the phase-`N−1` convention and output delay `@(j)` (line `j`,
0-indexed), the derivation gives, for `interleave(N, identity)`: sample
`x((k−1)N + i)` enters at `t = (k−1)N + i`, is exposed on line `i` at
tick `t = kN−1`, and comes out (option A) at `t = kN−1+i` — **constant
latency N−1**, i.e. `interleave(N, id) = @(N−1)`, and for `N = 2` we
indeed recover `mem`. The `2N−1` raised in the initial analysis came
from the paper's hold-with-offset semantics of `↑`; option A (fresh
value at the very tick via `Seq`) avoids it. **Locking milestone**: the
sample-by-sample unrolled table for `N=2` (columns
`t, x, H, serialize_in lines, fire, FX=id, up0, output`) as a structural
test + the runtime test `interleave(N, si.bus(N)) == @(N−1)` for several
N — this is S1 in the plan (§7).

Overlap (hop < N): the same construction gives **overlap-add for
free** — clock of period `hop`, `serialize_in` unchanged (overlapping
windows), and on the output side the `:>` summation of the delayed
zero-stuffed lines naturally adds the overlapping frames. The COLA
condition on the window remains the user's responsibility (provable,
later, as a theorem — out of compiler scope).

### 4.5 FFT, differentiability, and honest positioning

- **Total core reuse**: `an.c_bit_reverse_shuffle(N)` + `an.fftb(N)` are
  already spatial and tested;
  `fft_framed(N) = interleave(N, an.rtocv(N) : an.fftb(N))` rewrites
  nothing — the clocking harness changes, not the DSP. For
  **analysis-only** use (spectral loss), `serialize_out` is unnecessary:
  the bins held by the `PermVar`s at frame rate are directly consumable;
  only resynthesis (phase vocoder) requires `serialize_out` + OLA.
- **Built-in oracle**: the library's sliding FFT *is* the reference — at
  the alignment ticks, `interleave(rtocv : fftb)` must produce the same
  spectrum as sliding `rtocv : fft`. No external FFT needed.
- **Differentiability**: `fftb` = routing + real arithmetic → already in
  the FAD/RAD fragment. But a realistic spectral loss
  (`fad(loss ∘ |STFT| ∘ dsp(θ), θ)`) **crosses the boundary** (the
  parameters and the signal enter through `serialize_in`, outer domain)
  → requires **Phase B** (P5). Under Phase A one can already
  differentiate whatever lives entirely at frame rate. On the RAD side:
  the periodic clock is **constant-rate**, so the STFT is linear
  periodically time-varying — the **LPTV transpose** of the YOLO path
  ([yolo-linearize-once-rad-analysis-2026-05-21-en.md](yolo-linearize-once-rad-analysis-2026-05-21-en.md))
  covers `rad` through the STFT *before* the general P8 tape.
  Table-stakes numerical detail: `∂|X|` is singular at 0 → epsilon in
  the magnitude (`sqrt(R²+I²+ε)`).
- **Positioning** (to be held exactly as stated in any presentation):
  differentiable FFT + spectral loss has been a DDSP/JAX/PyTorch staple
  since 2020 — in the *offline, batch, GPU, reverse-mode* regime. The
  faust-rs contribution is not FFT differentiability; it is its
  integration into the **real-time synchronous** execution model:
  in-graph adaptation while audio plays, single source, multi-backend,
  no ML runtime. The decisive demo: a biquad whose coefficients descend
  the gradient of a spectral loss **live**, compiled to a dependency-free
  plugin.
- **Vector mode**: the STFT's periodic `ondemand` becomes a scalar
  island under D1 (correct, serial); and it is the **ideal D2
  candidate** — literal factor, fully stateless `fftb` interior → SIMD
  at frame rate.

### 4.6 Risks specific to the spectral track

1. **Compile-time cost**: unrolled `fftb(N)` = O(N log N) nodes + an
   N-pair bit-reversal `route`; `serialize_in(N)` = N lines with O(N)
   delays. For N = 1024–4096, genuine stress on the pattern matcher and
   generated code size. Stage it: N=4 → 64 → 1024 with measurements.
2. **Phase convention not locked** before S1 — everything else in track
   S depends on it (analysis/resynthesis alignment).
3. **Option A and non-boolean clocks**: documented restriction, with a
   diagnostic if `up0` is used under integer OD/US.
4. **`serialize_in` delays at the outer rate**: N−1 delay lines on the
   same signal — verify that multi-tap sharing (the `delay/` strategies)
   produces *one* buffer, not N.

## 5. Dependency overview (updated)

The roadmap's P0–P9 graph stays valid; the spectral track S attaches as
follows:

```
P0 ──→ P1 ──┐
            ├──→ P3 ──→ P4 ──→ P5 ──┬──→ P8 ──→ P9 (LPTV, TBPTT)
P2 ─────────┤        │              │
            │        ├──→ S1 ──→ S2 ──→ S3 ──→ S4 (S4 requires P5)
            └──→ P6 ─┤              │
                     └──→ P7 ───────┴──→ P9 (D2) ──→ S5
```

- **S1–S3 depend only on P3** (scalar OD lowering + C/C++ backends): as
  soon as the base port runs, the spectral track starts — without
  waiting for FAD Phase B or vector mode.
- **S4** (differentiable STFT) requires P5 (Phase B, boundary crossing).
- **S5** (performance) requires P7+P9-D2 (island-interior vectorization)
  and, for `rad`-STFT, the LPTV path (P9) or P8.

## 6. Updated P0–P9 implementation plan

The detailed contents (checklists) of the 2026-06-10 roadmap remain the
reference; below is what **changes** per phase — post-refactor file
targets, and adjustments.

### P0 — Guards & groundwork (S–M, **indivisible**, first)

Unchanged in substance (roadmap §3), targets refreshed:

- **P0.1**: pull `Clocked` out of the shared arm in
  `crates/transform/src/signal_prepare/verify.rs:257` (never visit the
  first child); add the missing
  `Seq`/`TempVar`/`PermVar`/`ZeroPad`/`OD`/`US`/`DS` arms in `verify.rs`
  and audit `rewrites.rs` + occurrences/CSE; clean `FRS-SFIR` rejection
  in `signal_fir` ("ondemand not lowered yet").
- **P0.2**: `ClockDomain { parent, kind, clock, instance }` arena
  replacing the cons tuple; fixes `make_clock_env`
  (`crates/propagate/src/engine.rs:1086`). Test: two structurally
  identical instances → distinct domains.
- **P0.3**: cache-key audit — **merged into the memoization work**
  (§2.3): the "key ⊇ clock_env" requirement goes into
  `cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md`, with the
  "same box under two domains → distinct signals" test.
- **P0.4**: replace the `_ => zero_tangent(sig)` of
  `crates/propagate/src/forward_ad.rs:1075` with an explicit arm for the
  glue → structured `FRS-PROP` error; name the construct in the RAD
  rejection (`reverse_ad.rs:331`). Snapshots for the four rows of the
  cohabitation §4 table.

### P1 — Clock-domain analyses (M): unchanged

Inference (`R_PROJ`/`R_CLOCKED`/`R_CD`/`R_SEQ`/`R_COMPOSITE`, Jacobi
fixed point, `SigId → ClockDomainId` side map) + `Hgraph`/`Hsched`
(audited partition, deterministic DFS toposort). New module
`crates/transform/src/clk_env.rs` (or a dedicated crate). Details in
roadmap §4.

### P2 — Region infrastructure in `signal_fir` (M, revised downward)

The W9 decomposition of `SignalToFirLower` and the `delay/` split
already do a good share of the preparatory work:

- **P2.1** region design note (unchanged): `Region` tree, single
  visibility rule ("a value computed in R is reusable only in R and its
  descendants; cross-region reuse goes through named storage"); FIR
  vocabulary decision (reuse the existing
  `If`/`SimpleForLoop`/`Block`, default confirmed by vector doc §4).
- **P2.2** diff-free refactor: replace the `sample_phases` accumulator
  (in `signal_fir/module/build.rs`) and per-bucket CSE (`cse.rs`,
  `placement.rs`) with the region tree instantiated with a single
  `SampleLoop` (+ the reverse-time loop as a sibling region).
  **Acceptance: zero diff on all goldens.**
- **P2.3** storage classes: `PermVar` → cleared struct fields, `TempVar`
  → parent-region locals, `IOTA`/`DSCounter` keyed by `ClockDomainId` —
  integrates into `signal_fir/delay/{manager,plan}.rs`.

### P3 — Scalar OD/US/DS lowering + first backends (L)

Unchanged (roadmap §6) with two updates:

- **P3.2**: structured emission of guarded blocks is written in the
  **shared C-family core** — one implementation for c and cpp, plus the
  named rejections for every other backend.
- **P3.4**: differential harness against the branch binary
  (`8eebea429`, faust 2.84.3), roadmap §6 corpus unchanged. Add at this
  stage the "boolean periodic clock of period N" fixture — the building
  block of track S.

### P4 / P5 — FAD Phases A and B (S–M / M): unchanged

Roadmap §7–§8. Reminders: Phase A = corpus only (zero new AD code) with
the six use-case families; Phase B = dual rules on the glue + `OD_aug`
memoized once per source block + tested `suppress_fad`/`ExpandAfterRec`
interplay + relaxation of the P0.4 diagnostic.

### P6 / P7 — Vector mode V1–V6 then D1 islands (L / M): unchanged

Roadmap §9–§10. Target update: vector delay strategies (V4) join as
`DelayKind` variants in `signal_fir/delay/`; V5 emission benefits from
the C-family core. Parallel track possible from the end of P2.

### P8 / P9 — RAD Phase C and optimizations: unchanged

Roadmap §11–§12. Added (P9): the LPTV transpose covers `rad` through
the constant-hop STFT (S4/S5) before the general tape.

## 7. Track S — the spectral track (new)

### S1 — Semantics and phase convention (S; depends on P3.1–P3.2)

- [ ] Sample-by-sample unrolled N=2 table (structural fixture) locking:
      clock phase (`t ≡ N−1 mod N`), output delays `@(j)`,
      "fresh-value-at-fire-tick" convention.
- [ ] Runtime test: `interleave(N, si.bus(N)) == @(N−1)` for
      N ∈ {2, 4, 16} (constant latency, identity up to a delay).
- [ ] `↑₀` decision **option A** (sugar `up0(H, y) = y * (H != 0)`)
      documented, with the boolean-clock restriction and its diagnostic;
      option B (`SigZeroVar`) recorded as the fallback with its trigger
      criteria (native RAD gradient, performance).
- [ ] Short design note in `porting/` (or rustdoc) if deviations from
      the present analysis appear.

### S2 — `interleave` library (S; depends on S1)

- [ ] Library (faust) definitions: `serialize_in(N)`, `up0`,
      `serialize_out(N)` (sum/OLA variant), `interleave(N, FX)`,
      periodic clock `frame_clock(N)` (phase N−1) and
      `frame_clock(N, hop)` for overlap.
- [ ] Impulse-tests fixture: `interleave(N, id)`, `interleave` with an
      internal delay (local IOTA), overlap hop = N/2 (OLA, simple COLA
      window).
- [ ] Verification of delay-line sharing in `serialize_in` (one buffer,
      not N — `delay/` strategy).

### S3 — FFT milestone (M; depends on S2)

- [ ] `fft_framed(4) = interleave(4, an.rtocv(4) : an.fftb(4))`
      validated against the sliding FFT of `analyzers.lib` at the
      alignment ticks (the oracle is in the library); end-to-end latency
      measured and checked against S1.
- [ ] Scaling in N: 64 then 1024, measuring compile time and generated
      code size (pattern-matcher/CSE stress); thresholds/alerts
      recorded.
- [ ] Analysis-only mode without `serialize_out` (bins held at frame
      rate) documented.

### S4 — Differentiable STFT (M; depends on S3 **and P5**)

- [ ] `fad` through `interleave` (seeds cross `serialize_in`): gradient
      of a twiddle made variable in `fftb(4)` vs finite differences.
- [ ] Magnitude spectral loss with epsilon (`sqrt(R²+I²+ε)`);
      convergence of a filter parameter on an `|STFT|` loss at frame
      rate (the cohabitation §2 case 4 restructured).
- [ ] Flagship demo: adaptive biquad with a spectral loss **in real
      time**, compiled to dependency-free C++ — the positioning
      artifact (§4.5).
- [ ] `rad`: snapshot of the named rejection as long as neither LPTV nor
      P8 covers the case.

### S5 — Spectral performance (M; depends on P7 + P9-D2)

- [ ] The STFT island under `-vec`: bit-exact vs scalar (D1), then the
      `fftb` interior vectorized at frame rate (D2 — ideal candidate:
      literal factor, stateless body).
- [ ] `rad`-STFT via the LPTV transpose (constant hop); tape/cost
      measurement.
- [ ] Throughput comparison sliding vs framed vs framed-vec (the
      O(N log N)/hop argument, quantified).

## 8. Flat landing order (single stream)

1. **P0** guards & groundwork — indivisible change set; the clock-env
   requirement is recorded in parallel in the memoization plan (§2.3)
2. **P1** inference + `Hgraph`/`Hsched`
3. **P2** regions (design, diff-free refactor, storage classes)
4. **P3** scalar lowering + C/C++ backends + SR/UI + differential
   harness — then P3.5 backends staggered
5. **P4** FAD Phase A (corpus)
6. **S1–S2** `interleave` semantics + library — *as soon as P3 holds;
   may precede P5*
7. **P5** FAD Phase B (block augmentation)
8. **S3** framed-vs-sliding FFT milestone
9. **S4** differentiable STFT + real-time demo
10. **P6** vector mode V1–V6 — *second stream possible from the end of
    P2*
11. **P7** D1 islands
12. **P8** RAD Phase C
13. **P9 + S5** optimizations (D2, LPTV, hoisting, TBPTT, spectral perf)

With two streams: {P0, P1} ∥ {P2}, then {P3, P4, S1–S3, P5, S4} ∥ {P6},
joining at P7, tail {P8, P9, S5}.

## 9. Consolidated risks (delta vs roadmap §13)

1. **FAD cliff** (unchanged, still open): P0 indivisible; no
   intermediate state may compile silently-zero gradients.
2. **Memoization × clock_env** (new, §2.3): to be locked into the
   memoization plan *before* its implementation lands.
3. **Unstable C++ reference** (unchanged): parity pinned to `8eebea429`;
   re-sync deliberate and journaled.
4. **FFT compile time** (new, §4.6): stage N=4 → 1024 with measurements;
   also a test bench for pattern-matcher optimizations.
5. **`interleave` phase convention** (new): nothing in track S beyond S1
   until the N=2 table is locked.
6. **TBPTT window under `-vec`** (unchanged): force-scalar for modules
   with a reverse-time loop until decided (P9).
7. **Interp backend** (unchanged): guarded-block + chunk-loop paths =
   the largest backend gap (P3.5, P6.6).

## 10. Validation (oracle surfaces)

| Topic | Oracle |
|---|---|
| Clock domains (base) | Differential vs branch binary `8eebea429` (impulse-tests, `cpp_signal_differential` style) |
| Vector mode (base) | **Bit-exact** scalar vs `-vec` within faust-rs + differential vs upstream `master` `-vec -lv 0|1` |
| `-vec` × domains (D1/D2) | Bit-exact scalar vs `-vec` within faust-rs (no upstream reference) |
| FAD/RAD × domains | Finite differences (`fad_recursive_runtime.rs` / `rad_runtime.rs` / `block_reverse_ad.rs` harnesses) — no C++ reference |
| `interleave` / framed FFT | Identity up to a delay (`@(N−1)`), then **the sliding FFT of `analyzers.lib` as reference** at the alignment ticks |
| Differentiable STFT | Finite differences + measured convergence of the real-time demo |
