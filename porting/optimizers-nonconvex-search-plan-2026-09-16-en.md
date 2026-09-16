# `optimizers.lib` beyond the basin: gradient-free search, multi-start, restart, noise, wider losses, initialisation helpers

Date: 2026-09-16

## Scope

Section 9 of `libraries/optimizers-overview-en.md` (added 2026-09-16) states
what gradient descent asks of a loss landscape and ends on what the library
does not offer: nothing that escapes a wrong basin, nothing that learns a
parameter `fad` cannot differentiate, no loss that widens a basin beyond
`energy_loss`, no helper that turns an outside estimate into an `init`. This
plan adds six things, in the library's own idiom (loops whose state is
`init + deviation`, engines `upd(g)`, clocked loops on `ondemand`, one
`#### Test` block per public function, a fixture and a Rust test per
behaviour, measured numbers in the documentation), each with an independent
check and a mutation the check must reject.

The six points, in the order they were argued in the discussion that led here:

| # | Point | Library surface |
|---|---|---|
| 1 | gradient-free engines at frame rate | `spsa_1D_clocked`, `search_1D_clocked`, `spsa_N_clocked` |
| 2 | multi-start and grid-then-gradient | `multistart_1D`, `grid_then_descend_1D` |
| 3 | restart on a plateau | `descend_1D_restart`, `stalled` |
| 4 | a noisy engine | `langevin_g` |
| 5 | losses that widen the basin | `bank_log_energy_loss`, `corr_loss`, `frame_spectral_loss` |
| 6 | initialisation and continuation helpers | `init_latch`, `init_pulse`, `ramp_lin`, `ramp_exp`, `stalled` |

Implementation order is by value over cost and by dependency: **N6 → N4 →
N1 → N3 → N2 → N5**. N6 is a day of helpers that N2 and N3 consume; N4 is one
line on top of N6; N1 is the only phase that changes the class of problems the
library reaches; N3 is the smallest addition that fixes the documented
counter-example; N2 is mechanical once the argmin fold exists; N5 is the
largest and the least certain in its numbers.

Library version: `0.9.0`, one `Changes in 0.9.0` note listing the six
additions, the two aliases kept bit-identical (`lr_exp` over `ramp_exp`), and
the changed count of `opt_all_functions.dsp`.

## The shared test bed

The waveguide string of `tests/corpus/ddsp_fad_waveguide_string_pitch.dsp` is
the one landscape the repository has measured: a well ±1 Hz wide around
220 Hz, residual rms 0.10–0.11 on the plateau from 150 to 300 Hz, capture from
228 Hz and from 264 Hz with annealed damping, drift from 200 Hz (→ 190) and
176 Hz (→ 168). Every phase that claims to escape a basin is measured on it,
from a start the current loop cannot recover from, with the same excitation,
the same bounds `[100, 400]` samples and the same NLMS engine unless the phase
is about the engine. A second bed, for what `fad` cannot see, is the same
string with an **integer** delay (`de.delay(MAXD, int(d))`): its tangent with
respect to `d` is zero by the library's own rule table, so only a
gradient-free method can tune it.

Numbers in this plan are **targets to measure, not results**: a phase is done
when its fixture prints the number, the Rust test asserts it, and the
documentation quotes it.

## Constraints inherited from the library

- **State form.** A parameter is `init + _dev(reset, prev)`, clipped in
  deviation space; `init` is a signal added outside the recursion
  (`descend_1D ... : +(init1)`), so a latched `init` with a `reset` pulse at
  the latch instant is a legal way to re-seed a loop without a first-sample
  detector. N2, N3 and N6 rest on this.
- **Engine contract.** `upd(g)` maps a gradient to a step and keeps its own
  state; the loops subtract it. N4 adds an engine; N1 adds loops whose `g` is
  an estimate, not a `fad` tangent, so the existing engines apply unchanged.
- **Clocked bodies.** Inside an `ondemand` body a recursion advances once per
  firing, a referenced definition is re-instantiated in the body's time (a
  noise generator inside a body advances once per firing, which N1 wants), a
  frame operator with free `_` inputs must receive named arguments, and
  outside signals enter as explicit inputs. `frame_sum`/`frame_mean` give
  exact per-frame reductions.
- **Documentation and tests.** Every public function has `Usage`, `Where`,
  `Test`, `References`; `tests/corpus/opt_all_functions.dsp` instantiates every
  `Test` entry and `every_documented_function_compiles_and_runs` asserts the
  output count (80 entries, 139 outputs today; both numbers change and the
  test's comment with them). Behavioural fixtures are `tests/corpus/opt_*.dsp`
  run by `crates/compiler/tests/optimizers_lib.rs`; DDSP-level fixtures are
  `tests/corpus/ddsp_*.dsp` run by `crates/compiler/tests/ddsp_examples.rs`.
  Documentation is bilingual and synchronized (`optimizers-overview-*.md`,
  `optimizers-ddsp-tutorial-*.md`, `ddsp-examples-*.md`), and the tutorial's
  quoted numbers are enforced by `crates/cranelift-ffi/tests/tutorial_examples.rs`.
- **Lists.** Faust has no lists. A family of `K` initial values is a function
  `init(k)` of a constant index, used under `par(k, K, ...)`, exactly as the
  loops take `loss` as a function; a user who wants explicit values writes
  `init(k) = ba.selectn(K, k, (v0, v1, ...))`.
- **Compile time.** Forty forward tangents through a small reverberator cost
  15 s of compilation. `K` copies of a differentiated model multiply the
  graph by `K`; N2 measures compile time at `K = 8` on the string and records
  it.

## N6 — Initialisation and continuation helpers (first: consumed by N2, N3, N4)

**Status 2026-09-16: landed** (`optimizers.lib` 0.9.0). Two deviations from
the text below. `init_pulse(T)` became `init_reset(T)`, 1 up to and
including sample `T`: holding the loop at `init` while the estimate is
observed is what the use case needs, and a one-sample pulse would let the
loop descend on a moving `init` before the latch. And a compile-time trap
surfaced: a recursion whose body closes over a large term (the estimate)
re-lowers it at every mention, 34 s for an 8-lane estimate feeding
`lsq_1D`, growing 4 s per lane, 0.07 s for the same graph outside a
recursion; the three 1D loops now take `init` through an input wire of
their recursive block (0.46 s at 8 lanes, 1.5 s for the shipped 30-lane
fixture), the multi-parameter loops still close over theirs, and the
compiler issue is filed as its own task. Measured: init frozen at
222.77 Hz, pitch 219.998 at 24 000 samples, 220.000000 from 48 000 on.


**Surface** (section "Signal Helpers and Parameter State", and "Gradient
Conditioning and Schedules"):

- `init_latch(T, estimate)`: `estimate` while `time < T`, then the value
  seen at sample `T`, held forever. `time` is the library's own counter
  (the `_first` idiom generalised to a count), so it works inside an
  `ondemand` body in the body's time.
- `init_pulse(T)`: 1 at sample `T`, 0 elsewhere. `(init_latch(T, e),
  init_pulse(T))` is what a loop takes as `(init, reset)` to start from an
  outside estimate after `T` samples of observation. *(Landed as
  `init_reset(T)`, held up to `T`; see the status note.)*
- `ramp_lin(from, to, T)`, `ramp_exp(from, to, T)`: the signal that goes
  from `from` to `to` in `T` samples, linearly or with time constant `T`.
  `lr_exp(lr0, lr_inf, T)` becomes an alias of `ramp_exp`, bit-identical.
  A ramp is a signal, so it also anneals a *model* parameter, which the
  string example does by hand with its damping (0.70 → 0.95).
- `stalled(a, eps_g, eps_l, g, l)`: 1 when `ema(a, |g|) < eps_g` while
  `ema(a, l) > eps_l`, the plateau detector N3 uses. Documented with its
  failure mode: a loss whose floor is above `eps_l` (noise floor) reads as a
  permanent stall; the user sets `eps_l` above the floor.

**Fixtures and checks.**

- `opt_init_latch_string.dsp`: the string, `init = init_latch(T, estimate)`
  where `estimate` is `an.pitchTracker(N, tau)` on the target
  (analyzers.lib, zero-crossing based, present in the installed standard
  library); a noise-driven string may defeat a zero-crossing tracker, in
  which case the fixture computes an autocorrelation-peak estimate itself and
  the documentation says which one it uses; `reset = init_pulse(T)`; target:
  lock on 220 Hz from a start of 176 Hz, which the plain loop cannot do.
  Measured number: pitch after 60 000 samples.
- Identity check: `opt_ramp_alias.dsp` outputs `lr_exp(...)` and
  `ramp_exp(...)` side by side; the Rust test asserts bit-identical lanes,
  and `descend_1d_with_adam_learns_a_gain` (which uses `lr_exp` nowhere) plus
  the biquad fixture (which does) keep their measured numbers.
- Mutation the checks must reject: `init_pulse` one sample late (the
  deviation is reset before the latch, the loop restarts from the *old*
  estimate) → the string fixture fails its lock.

## N4 — A noisy engine: `langevin_g`

**Surface** (section "Gradient Engines"):
`langevin_g(lr, temp, noise, g) = lr * g + sqrt(2 * lr * temp) * noise`,
with `temp` a signal (a `ramp_exp` to zero is the annealing schedule) and
`noise` any unit-variance signal (`no.noise` is uniform on `[-1, 1]`, variance
1/3; the documentation says so and the `Test` entry scales it). Reference:
Welling & Teh 2011 (SGLD). Documented limit, stated in the overview: noise
leaves a shallow well; it does not pull on a flat plateau.

**Fixtures and checks.**

- `opt_langevin_two_wells.dsp`: the synthetic loss
  `l(p) = (p² − 1)² + 0.3 p`, two wells, the right one shallower; from
  `p = +1` plain `sgd_g` stays at the shallow well, `langevin_g` with
  `temp = ramp_exp(0.5, 0.0, 20 000)` ends in the deep well at `p ≈ −1.07`.
  The test asserts the final mean over the last 10 000 samples for both
  lanes. This loss has no data, so the fixture is deterministic given the
  noise generator's seed.
- Identity check: with `temp = 0`, `langevin_g(lr, 0, n, g)` is
  bit-identical to `sgd_g(lr, g)`; asserted on `opt_langevin_two_wells.dsp`'s
  third lane.
- Mutation: drop the `sqrt` → the noise amplitude is `2 lr temp`, orders of
  magnitude smaller at `lr = 0.01` → the escape fails; the identity check
  still passes, which is why the escape check exists.

## N1 — Gradient-free engines at frame rate

**What it changes.** Everything the library learns today goes through a
`fad` or `rad` tangent; nodes without a rule (integer arithmetic, `int`
casts, table writes, buttons) give a zero tangent, and a `select2` gives the
active branch only. A loss evaluated twice per frame needs no tangent at all,
so an integer delay length, a discrete choice or a written table become
learnable, at frame rate, for two model evaluations instead of one model and
its tangent.

**Surface** (new section "Gradient-Free Loops", between "Clocked Loops" and
"Gating and Stopping"):

- `spsa_1D_clocked(clock, loss, upd, c, lo, hi, init, reset)`: simultaneous
  perturbation (Spall 1992). The block outputs the parameter `p` and the
  perturbed pair `p ± c·Δ`, held over the frame; at audio rate the fixture
  evaluates `loss(p + cΔ)` and `loss(p − cΔ)` (two model copies on the same
  excitation: common random numbers, which is what makes the difference
  informative on a noisy loss), `frame_mean` reduces both; on the firing the
  body forms `g = (L₊ − L₋) / (2 c Δ)` and hands it to `upd`, the ordinary
  engine contract. `Δ` is a Rademacher sign drawn from an LCG *inside* the
  body, hence once per firing. `c` may be a ramp (`ramp_exp`) as Spall
  recommends.
- `search_1D_clocked(clock, loss, sigma, lo, hi, init, reset)`: the (1+1)
  evolution strategy (Rechenberg 1973): propose `p + σ·n` for the next frame,
  keep it if its frame loss is lower than the incumbent's, else return; with
  the 1/5 success rule on `σ` as an option in the documentation, not in the
  first version. No engine: the step is the acceptance.
- `spsa_N_clocked(N, clock, loss, upd, c, lo, hi, init, reset)`: the bus form,
  one perturbation vector per frame, the same two evaluations whatever `N`
  (the point of SPSA over coordinate-wise finite differences).

**Fixtures and checks.**

- `opt_spsa_int_delay_string.dsp`: the integer-delay string, `d` in
  `[100, 400]`, from the delay of 228 Hz, frames of 256; target: the delay
  reaches `round(SR / 220)` and stays. `fad` on this model gives a zero
  tangent (asserted in the same fixture on a lane: `fad(mdl(d), d) : !, _`
  is identically zero), so the lock can only come from SPSA.
- `opt_spsa_vs_fad_gain.dsp`: on a smooth loss (a gain), the SPSA loop and
  `descend_1D_clocked` with the same engine and clock run side by side; the
  test asserts both reach 0.7 and that the SPSA gradient lane's frame mean
  agrees with the `fad` gradient's frame mean to within the tolerance the
  fixture prints (an estimate, not an identity). This is the independent
  check of the estimator.
- `opt_search_select2.dsp`: a model with a discrete choice inside
  (`select2(p > 0.5, A, B)` where only `B` matches the target) learned by
  `search_1D_clocked` from `p = 0`; `descend_1D` on the same loss provably
  never moves (zero tangent through the comparison), which the fixture shows
  on a second lane.
- Mutations: (a) the perturbation sign of one branch flipped (`p + cΔ` and
  `p + cΔ`) → `g ≡ 0` → the int-delay fixture fails; (b) `Δ` drawn at audio
  rate instead of per firing → the two copies are perturbed inconsistently
  within the frame → the vs-`fad` agreement fails.
- Measured and recorded: compile time and per-frame cost of the two copies
  against one `fad` copy on the float string.

## N3 — Restart on a plateau

**Surface** (section "Loss-First Loops"):
`descend_1D_restart(K, init, loss, upd, lo, hi, a, eps_g, eps_l, reset)`:
`descend_1D` whose `init` is `init(k)` for a restart index `k` counted from 0;
`stalled(a, eps_g, eps_l, g, l)` advances `k` (modulo `K`) and pulses an
internal reset OR-ed with the user's. A refractory window after each restart
(the `ema` horizon) prevents an immediate second trigger. Outputs: the
parameter and `k`, so a host or a bargraph sees the restarts. Cost: one model,
sequential search over `K` starts.

**Fixtures and checks.**

- `opt_restart_string.dsp`: the string from 200 Hz (documented drift to 190)
  with `init(k) = ba.selectn(2, k, (SR/200, SR/240))`; target: the first
  start stalls on the plateau, the restart from 240 Hz is captured from
  above, the pitch locks on 220 within a measured number of samples; the `k`
  lane ends at 1.
- Check of the detector alone: `opt_stalled_lanes.dsp` feeds `stalled` with
  synthetic `(g, l)` sequences (a converging pair, a plateau pair, a
  noise-floor pair) and the test asserts its 0/1 lane per segment.
- Mutation: `stalled` returns 0 always → `k` stays 0, the pitch drifts to
  190 → the fixture fails on both lanes.

## N2 — Multi-start and grid-then-gradient

**Surface** (section "Loss-First Loops"):

- `multistart_1D(K, init, loss, upd, lo, hi, a, reset)`: `K` `descend_1D`
  loops in parallel from `init(k)`, each with `ema(a, loss(p_k))`, an argmin
  fold (`seq` over `K − 1` comparators carrying `(best_l, best_p, best_k)` and
  `select2(l_k < best_l, ...)`), outputs `(p_best, k_best)`. Cost `K` models;
  the fold is written once as `argmin_K` in the signal helpers.
- `grid_then_descend_1D(K, T, init, loss, upd, lo, hi, reset)`: for `T`
  samples, `K` **undifferentiated** models at fixed `init(k)` and a running
  sum of each loss (no tangent, cheap); at `T`, `argmin_K` is latched by
  `init_latch(T, ...)` into the `init` of one `descend_1D`, reset by
  `init_pulse(T)`. After `T` the `K` fixed models are dead code only if the
  compiler removes them; the documentation says what remains and N2 measures
  it. Default `init(k) = lo + (k + 0.5) (hi − lo) / K`, exported as
  `grid_init(K, lo, hi)`.

**Fixtures and checks.**

- `opt_grid_string.dsp`: `grid_then_descend_1D` with `K = 8` over
  `[100, 400]` samples (110 to 441 Hz), `T = 8 192`; target: the grid picks
  the cell containing 220 Hz and the descent locks on 220.000 from any of
  the three documented failing starts, since the start no longer matters.
- `opt_multistart_string.dsp`: `multistart_1D` with `K = 4` from
  `(176, 200, 228, 264 Hz)`; target: `k_best` ends at 2 (228) or 3 (264 with
  the annealed damping), `p_best` on 220.
- Independent check of the fold: `opt_argmin_lanes.dsp` feeds `argmin_K`
  with `K` synthetic loss lanes whose minimum moves over time and the test
  asserts the index lane against the minimum computed in Rust from the same
  lanes.
- Mutation: comparator inverted (`>` for `<`) → `argmin` selects the worst
  → both lane tests fail.
- Measured and recorded: compile time and instruction count at `K = 8` on
  the differentiated string (multistart) against `K` undifferentiated copies
  (grid), which is the argument for the grid form.

## N5 — Losses that widen the basin

**Surface** (section "Losses and Regularizers"):

- `corr_loss(a, eps, y, t) = −ema(a, y t) / sqrt(ema(a, y²) ema(a, t²) + eps)`:
  normalised correlation, scale-free, mentioned by the string example as
  removing the group-delay bias; gradient through `ema` is the library's
  existing smoothed form.
- `bank_log_energy_loss(B, lo, hi, a, eps, y, t)`: `B` band-pass bands
  (`fi.resonbp`, log-spaced from `lo` to `hi`, Q fixed by the spacing),
  `log_energy_loss` per band, summed. The filter-bank approximation of the
  multi-resolution spectral loss, at audio rate, without `ondemand` or FFT:
  the workhorse of DDSP (Engel et al. 2020) in the form section 8 of the
  overview says filter banks approximate.
- `frame_spectral_loss(N, y..., t...)`: the per-frame FFT magnitude loss of
  `tests/corpus/ondemand_fad_spectral_loss_008.dsp`, promoted to the library
  as a frame operator with named inputs (the rule of clocked bodies); the
  multi-resolution version is two blocks of different `N` summed, shown in
  the tutorial rather than wrapped, since a block's arity is its frame size.

**Fixtures and checks.**

- Landscape scan before any learning: `opt_landscape_string.dsp` exposes `d`
  as a slider and outputs the three losses (`mse`, `corr_loss`,
  `bank_log_energy_loss`) at fixed `d`; a host loop (`faustprobe --set`) scans
  150–300 Hz in 1 Hz steps and prints the three profiles. This is the check:
  the bank loss's well must be measurably wider than ±1 Hz, and the plan
  records the measured width. If it is not wider, N5 stops there and the
  overview says so.
- `opt_bank_loss_string.dsp`: the string learned through
  `bank_log_energy_loss` from 200 Hz; target set from the scan (capture from
  wherever the scan shows a monotone slope toward 220).
- Symmetry and zero checks on synthetic signals: `loss(y, t) == loss(t, y)`
  bit-identical for the two symmetric losses (`corr_loss` and the bank loss),
  `loss(t, t)` at the floor `eps` for the bank loss and `−1` for `corr_loss`.
- Tangent check: `fad` of the bank loss with respect to a gain against a
  finite difference from the host, the pattern of the FFT-loss fixture.
- Mutation: a band's centre frequency computed in linear spacing → the
  symmetry check passes, the scan's well width changes → the recorded width
  is the reject, which is why it is recorded as a number.

## Documentation per phase

- Overview: the tables of section 3 (surface) and 4 (origin of each
  algorithm, with the references below added to section 10), section 5
  (measured behaviour: one line per fixture), section 9 rewritten from "none
  of this is in the library" to what exists, with the measured string
  captures, and section 6 for the new pitfalls (stall on a noise floor,
  `K` compile time, SPSA on a loss with a noise floor smaller than `c`).
- Tutorial: a section "14. When the start is wrong" walking the string from
  200 Hz through restart, grid and SPSA, every number measured with
  `faustprobe` and enforced by `tutorial_examples.rs`; rows in "13. Frequent
  walls" (parameter never moves although the loss is high → it is not
  differentiable, use `spsa`/`search`; drifts away from the target → wrong
  basin, use restart/grid).
- DDSP examples: example 10's "Try" line points to the three fixtures; a
  thirteenth example only if N1 on the integer-delay string is worth its
  own page.
- References: Spall 1992 (SPSA), Rechenberg 1973 and Beyer & Schwefel 2002
  (evolution strategies), Welling & Teh 2011 (SGLD), Martí 2003 (multi-start),
  Engel et al. 2020 already present for the spectral loss.

## Gates per phase

1. `cargo test -p compiler --test optimizers_lib` and `--test ddsp_examples`
   green, including the new fixture tests and the updated output count of
   `every_documented_function_compiles_and_runs`.
2. Existing fixtures unchanged in their measured numbers; the two aliases
   bit-identical (`opt_ramp_alias.dsp`, the `temp = 0` lane).
3. The phase's mutation applied by hand once, its test seen to fail, and the
   fact noted in the journal entry (the library is Faust, so no Rust mutation
   harness; the manual step is the record).
4. `crates/cranelift-ffi/tests/tutorial_examples.rs` green after the tutorial
   section is written.
5. Both languages of every touched document updated in the same commit.
6. Journal entry per phase in `porting/journal/`, in English.

## Out of scope, and why

- CMA-ES and Nelder-Mead: a covariance matrix or a sorted simplex of `K`
  points; Faust has neither matrices nor sorting beyond a fixed fold. SPSA
  and the (1+1)-ES cover the gradient-free need at the library's scale.
- Bayesian optimisation and hyperparameter search: host-side, on the loop of
  `docs/rad-usage-en.md`; a later note may give a reference host program.
- Second-order on the loss (Newton on non-convex losses, trust regions): no
  second derivative exists (`fad` over `rad` is refused); `lm_2D`/`lm_3D`
  remain the second-order surface.
- Restart and multi-start in `N` dimensions: the 1D forms first; the bus form
  follows the same fold once the 1D numbers are in.
