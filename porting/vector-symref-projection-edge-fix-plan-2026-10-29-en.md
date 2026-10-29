# Vector SYMREF Projection Edge Fix Plan

Date: 2026-10-29
Status: complete (Y1 tests, Y2 fix and qualification landed 2026-10-29)
Scope: the scheduling edge of a symbolic back-reference projection
`SIGPROJ(i, SYMREF(W))`, and the X2b implicit-history machinery built on it
Trigger: GitHub issue grame-cncm/faust-rs#24 ("-vec breaks my dsp")

## 1. Defect

Under `-vec`, a recursion slot whose body is a bare projection of another
recursion group (an *alias slot*) is fed back with two samples of delay
instead of one. Stdlib-free reproducer:

```
process = (f ~ _) with { f(prev) = (+(prev*0.1+1) ~ *(0.5)); };
```

Scalar (and Faust C++ 2.90 `-vec`): `y[n] = 1 + 0.6 y[n-1]` (1, 1.6, 1.96...).
Rust `-vec`: `y[n] = 1 + 0.5 y[n-1] + 0.1 y[n-2]` (1, 1.5, 1.85...), for every
vector size and loop variant. The pipeline certifies the program, so the
miscompile is silent. The dependency need not be mutual:
`(f ~ (_,_)) with { f(a,b) = (+(1) ~ *(0.5)), a*0.5; }` is wrong too.

`pm.ks(200, 0.5)`, the X2b regression instance, is miscompiled in the same
way from frame 395 on. Its test renders 64 frames, under the ~200-sample
delay, and passes.

## 2. Root cause

The prepared tree encodes `~` as `SIGDELAY1(SIGPROJ(i, SYMREF(W)))`: the
one-sample delay is explicit (`propagate`, and the signal API's `CsigSelfN`,
as C++ `sigSelf = sigDelay1(sigProj(0, ref(1)))`). A bare `SIGPROJ(i,
SYMREF(W))` is the *current* value of slot `i`.

`vector/analysis/dependencies.rs::projection_dependency` nevertheless gives
`SIGPROJ(i, SYMREF(W)) -> body_i` the kind `Delayed { amount: 1 }`, on the
stated ground that SYMREF "denotes the implicit one-sample back-edge". C++
`getSignalDependencies` (`compiler/Dependencies/DependenciesUtils.cpp`) makes
`proj(i, rec) -> nth(body, i)` an **immediate** dependency; only the
`sigDelay1` above it is delayed. The Rust edge double-counts the delay.

For a self-recursion the extra delay on the edge is harmless to values (it
only relaxes ordering). For an alias slot it is not:

- X1 class B: the `Delayed` edge targets a body record with `max_delay = 0`,
  so neither a transport nor a delay transition serves it.
- X2b (`3ac62e8d`) made the state facts agree with the edge instead of fixing
  it: P6.1 (`state/build.rs::effective_delay_requirements`, mirrored by
  `state/check.rs::independently_expected_delay_requirements`) gives the body
  a one-sample history, and `lower/signal.rs::lower_projection` lowers the
  alias projection as `body@1`. The alias's own delay line is then written
  with `body[n-1]` and read through the explicit `SIGDELAY1`: `body[n-2]`.

The checker could not object: it re-derives the same obligation from the same
edge. The certificate was consistent and wrong.

## 3. Correction (Y2)

1. `projection_dependency`: every `SIGPROJ` of a symbolic recursion, SYMREC or
   SYMREF, schedules its selected body `Immediate` (C++ parity). The cycle
   through a recursion stays broken by the explicit `SIGDELAY1`/`SIGDELAY`
   edge, as in C++.
2. Remove X2b: the P6.1 widening in the producer and the checker, and the
   implicit-history branch of `lower_projection`. With (1) no `Delayed` edge
   leaves a recursive projection, so these are dead code encoding the wrong
   semantics.
3. Hand-built unit-test trees that use a bare `SIGPROJ(i, SYMREF)` as the
   feedback read are ill-formed for C++ (an instantaneous cycle, which the
   scalar `hgraph` now reports) and are rewritten to the `SIGDELAY1` form
   every real producer emits.

The scalar `-ss` schedule (`hgraph`) consumes the same edges, so (1) may
reorder scalar code toward C++ parity; it must not change scalar values.

## 4. Independence

The edge kind is shared by producer and checker, so moving it moves both
(the E0 situation). Independent evidence comes from outside the certificate:

- the numeric oracle: scalar vs vector bit-exactness over renders longer than
  every delay in the program;
- the pinned C++ rule, as an analysis unit test (projection `Immediate`,
  `SIGDELAY1` `Delayed{1}`).

## 5. Tests (Y1, written first, failing on the base)

- `crates/compiler/tests/vector_mode.rs`: stdlib-free alias reproducers
  (nested alias, independent alias, two-level alias chain), scalar vs vector
  bit-exact over a render longer than their delays, all strategies, both loop
  variants, two vector sizes.
- The Karplus regression renders past `pm.ks`'s delay (1024 frames).
- Analysis unit test: back-reference projection edge is `Immediate`.
- Mutation: restoring `Delayed{1}` on the back-reference edge must fail the
  new bit-exact tests.

## 6. Qualification

- Issue DSP (`hermiteAttackReleaseSmootherNoShape.dsp` at `1c113e1d`): six
  outputs bit-identical scalar vs `-vec` over 96,000 frames, still
  vectorized.
- `tests/impulse-tests/dsp`: scalar vs `-vec` long renders, certification
  counts before/after.
- Base-vs-new FIR differential over `tests/` and `examples/` (scalar, `-vec`);
  every changed emission explained.
- Workspace gates, structure gates, compile-cost A/B by hand (the local
  calibration floor prevents `compile-budget-check` from giving a verdict).

## 7. Results

- Y1: the three alias reproducers and the 1,024-frame Karplus check fail on
  the base (`68f03ec2`) and pass after Y2. Restoring `Delayed{1}` on the
  back-reference edge fails four tests: the two vector tests (by falling back,
  "no planned transport", not by miscompiling) and both analysis tests.
- Issue DSP: six outputs bit-identical scalar vs `-vec` over 96,000 frames
  under five control settings (faustprobe), and the same output hash for
  native C++ scalar and `-vec` (`-O3 -ffp-contract=off`); the base `-vec` hash
  differs. Still vectorized.
- `pm.ks` and the reproducers: certified and bit-exact at `-vs` 32, 24 and 7.
- Emission differential over the 632 `.dsp` of `tests/` and `examples/`
  (37 fail to compile on both binaries):
  - scalar: 100 emissions change, from the `hgraph` schedule consuming the
    corrected edge; base and new render bit-identically for all 100 (white
    noise and impulse, 20,000 frames);
  - `-vec`: 60 emissions change; no program gains or loses vectorization;
    new `-vec` equals new scalar bit for bit for all 60. Seven of them were
    miscompiled by the base `-vec` and certified anyway:
    `ddsp_fad_string_self_tuning`, `ddsp_fad_waveguide_string_pitch`,
    `opt_bank_loss_string`, `opt_init_latch_string`, `opt_landscape_string`,
    `opt_multistart_string`, `rep_71_degenerate_unary_recursion`.
    `ondemand_stft_denoiser_1024 -vec` exceeds the time limit on both.
- Impulse oracle against the C++ references, `.ir` caches deleted first:
  `cpp`, `cpp-vec0`, `cpp-vec1`, `interp-vec0` each ran 93 comparisons, all
  equal.
- Compile cost (basket, best of 5 interleaved): new/base between 0.984 and
  1.022 on every case.
- `vector-coverage-check` cannot give a verdict on this machine: it imports
  the installed `/usr/local/share/faust`, whose `sinh` is a foreign function,
  and stops on `phaser_flanger` (same failure on the base).

## 8. Remaining risk

The dormant LTI reverse path (`transpose_ad.rs`, `ReverseTimeRec`) writes the
adjoint feedback as a bare `SIGPROJ(i, DEBRUIJNREF(1))` meaning "the next
frame's state". It is `#[allow(dead_code)]` and reached only from tests. If it
is revived it must emit an explicit delay, or the analysis must model the
reverse-time back-edge separately; with this fix a bare back-reference is an
instantaneous cycle, which the scalar `hgraph` rejects.
