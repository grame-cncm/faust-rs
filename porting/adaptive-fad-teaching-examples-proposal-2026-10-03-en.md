# `adaptive_fad` teaching examples: proposal (2026-10-03)

## 1. Context

`optimizers.lib` 0.11.0 ships `adaptive_fad` / `adaptive_rad`, a whole
program learning its own control inputs without being rewritten. They are
built on the control-input primitives `cinputs(e)` and `cinput(i, e)` and on
the wildcard modulation target `"*"` (branch `control-inputs-wildcard`,
`docs/control-inputs-en.md` §3.1). Neither the beginner tutorial
(`libraries/optimizers-ddsp-tutorial-{en,fr}.md`) nor the DDSP example set
(`libraries/ddsp-examples-{en,fr}.md`) covers them. The only end-to-end use
today is the sibling project faust-diff-ampmodeler, outside this repository.

This document proposes two examples:

- a **simple** one, a new tutorial section that introduces the operator on a
  two-slider program;
- an **ambitious** one, DDSP example 15: a six-slider effect that learns its
  whole setting from a recording.

All the figures below were measured with `faustprobe` (release) on the
rebased `control-inputs-wildcard` (HEAD `64ca84d5` on top of `main-dev`
`24f9c953`).

## 2. Deliverables and pass criteria

| # | Deliverable | Pass criterion |
|---|---|---|
| D1 | Tutorial §13 "Learning the sliders of an existing program", EN and FR, three programs | `tutorial_examples.rs`: the French tutorial carries the same programs (count 33 → 36); one test per program checks the figures quoted in the text |
| D2 | `tests/corpus/ddsp_fad_adaptive_pedal.dsp` + test in `crates/compiler/tests/ddsp_examples.rs` | f32 interpreter: the six sliders start at their defaults and end within 1e-3 (relative) of the hidden setting; residual < 1e-5 rms over the last 20 000 of 200 000 samples. **Done, passing** (residual 7.1e-7, 12 s in debug). |
| D3 | `ddsp-examples-{en,fr}.md` §15, title, summary table, "where the optimizer runs", "how the tests check" | Figures quoted = figures the D2 test checks, with margins |
| D4 | Cross-references: `libraries/README.md` ("fourteen"), the tutorial's "Going further" list, the tutorial's "Common walls" table (one row, §5.1) | Wording consistent across EN/FR |
| D5 | Journal entry `porting/journal/2026-10-03.md` + index | Per AGENTS.md §11 |

Gates: `cargo fmt`, `clippy -D warnings`, `cargo test --workspace`, the five
structure gates, `xtask golden-check`. Golden-check already passes with the
new fixture: like the other `ddsp_*` fixtures that import the standard
libraries, it has no golden. The compile budget is unaffected (docs and tests
only).

## 3. The simple example: tutorial §13

**Placement.** A new §13, after "When the start is wrong" (§12) and before
"Going further". The current §13 and §14 become §14 and §15. No test refers
to them by number.

**Story.** Until §12 every parameter was a function argument. A real program
has sliders. §13 shows that the operator turns them into parameters, and that
the pitfall of §5.1 (one rate for two units) comes back, with its remedy read
from the sliders themselves.

The model is two lines, written as a player would write them:

```faust
e = fi.lowpass(1, hslider("cutoff", 1000, 50, 5000, 1)) : *(hslider("gain", 0.5, 0, 2, 0.01));
```

The target is the same chain at 2500 Hz and gain 1.2, on `0.3 * no.noise`.
`clock = (ba.time % 256) == 255`. The section has three programs.

**13.1 What the program exposes.** No learning yet:

```faust
process = outputs(cinputs(e)), par(i, outputs(cinputs(e)), cinput(i, e) : !, si.bus(4));
```

With `-n 1` it prints `2, 1000, 50, 5000, 1, 0.5, 0, 2, 0.01`: the number of
controls, then default, min, max and step for each control, in interface
order (cutoff, gain). The prose explains `["*": (!, _) -> e]` on this
output.

**13.2 One rate for all (the form of the library's documentation).**

```faust
process = op.adaptive_fad(e, op.mse, op.adam_g(0.01, 0.9, 0.999, 1e-8), clock, 0, x, target)
        : \(y, c, g).(c, g, y - target);
```

| samples | cutoff (Hz) | gain | |
|---|---|---|---|
| 10 000 | 1000.40 | 0.875 | |
| 50 000 | 1002.00 | 1.585 | |
| last 10 000 of 80 000 | 1002.9 | 1.634 | residual 0.035 rms |

The cutoff moves 0.01 Hz per step. The gain overshoots 1.2 to make up for
the missing treble: a wrong compromise, the §5.1 lesson on a real program.

**13.3 One rate per slider, read from the slider.**

```faust
N = outputs(cinputs(e));
range(i) = cinput(i, e) : !, !, \(lo, hi).(hi - lo), !;
upd = par(i, N, op.adam_g(0.01 * range(i), 0.9, 0.999, 1e-8));
process = op.adaptive_fad(e, op.mse, upd, clock, 0, x, target) : \(y, c, g).(c, g, y - target);
```

| samples | cutoff (Hz) | gain |
|---|---|---|
| 10 000 | 2574.69 | 1.1752 |
| 40 000 | 2500.47 | 1.1998 |
| 60 000 | 2499.98 | 1.200001 |
| last 10 000 of 80 000 | 2500.0005 | 1.1999997 (residual 2.4e-8 rms) |

**Closing prose.** Five points:

- the outputs are those of `e` followed by the parameters;
- the rebound sliders leave the interface;
- `reset`;
- the host-driven form `fad(loss, cinputs(e))`;
- `adaptive_rad` for many controls, with a pointer to DDSP example 15 for its
  limit on recursive models.

**Common walls table, one new row.** Symptom: "a learned slider jumps to its
bound on the first step". Cause: the model goes through a function that is
not differentiable at the slider's default, for example `fi.peak_eq`, which
takes `abs` of its gain, at 0 dB (finding F1). Remedy: a smooth equivalent
(`fi.peak_eq_rm`) or another default.

## 4. The ambitious example: DDSP example 15, "a pedal that learns its setting from a recording"

**Task an audio engineer recognises.** Recall the setting of a drive pedal
from a recording of it. The program is an ordinary effect with six sliders in
their own units, under an `hgroup`. Nothing in it mentions learning:

```faust
pedal = hgroup("pedal",
      fi.highpass(1, hslider("tight", 80, 20, 400, 1))
    : *(ba.db2linear(hslider("drive", 12, 0, 30, 0.1)))
    : ma.tanh
    : fi.lowpass(1, hslider("tone", 3000, 500, 8000, 1))
    : fi.peak_eq_rm(hslider("mid_gain", 0, -12, 12, 0.1), hslider("mid_freq", 800, 200, 3000, 1), tan(ma.PI * 400 / ma.SR))
    : *(ba.db2linear(hslider("level", -12, -40, 0, 0.1))));
hidden = ["tight": 150, "drive": 20, "tone": 1800, "mid_gain": 5, "mid_freq": 1200, "level": -6 -> pedal];
```

**What it shows beyond §13:**

1. **The program is untouched.** The learning wraps a program written for a
   player, groups included.
2. **The target comes from the same program** through named literal
   modulations. This is the second use of modulation in the file, next to the
   `"*"` inside the operator.
3. **Six parameters in three units** (dB, Hz, dimensionless), each with its
   own rate read with `cinput`.
4. **A nonlinearity between two gains.** Drive and level are separable only
   because the envelope (0.1 to 1) drives the tanh at several depths. This is
   identifiability, as section 6 of faust-diff-ampmodeler discusses: a
   constant-level excitation would let them trade.
5. **`fad` against `rad` on a recursive model** (§4.2).

### 4.1 Measured (double; f32 agrees to 1e-5)

`clock = (ba.time % 512) == 511`, Adam at 1 % of each slider's range. The
excitation is `0.3 * env * (0.7 * os.sawtooth(110) + 0.3 * no.noise)` with
`env = 0.55 + 0.45 * sin(2 pi phasor(1.3 Hz))`.

| samples | drive (dB) | level (dB) | mid_freq | mid_gain | tight | tone |
|---|---|---|---|---|---|---|
| 0 (defaults) | 12 | −12 | 800 | 0 | 80 | 3000 |
| 25 000 | 18.29 | −4.58 | 1191.1 | 5.18 | 139.7 | 2707.1 |
| 50 000 | 19.00 | −5.60 | 1203.6 | 5.35 | 145.2 | 1845.4 |
| 100 000 | 19.95 | −5.96 | 1199.4 | 4.97 | 149.6 | 1808.7 |
| 150 000 | 19.9998 | −6.0001 | 1199.999 | 4.9998 | 149.998 | 1800.06 |
| last 20 000 of 200 000 | 20.000006 | −5.999998 | 1199.99997 | 5.000002 | 150.0001 | 1799.998 |
| hidden | 20 | −6 | 1200 | 5 | 150 | 1800 |

- Residual: 2.0e-7 rms in double and 7.1e-7 rms in f32, over the last 20 000
  samples.
- Cost: 126x real time (`faustprobe --time`, 10 s of audio).

**Tuning chosen.** A 1024-sample frame at 1 % converges too, but in about
400 000 samples. A 2 % rate gets there sooner, with overshoots (drive 20.08,
tight 163 Hz). A 512-sample frame at 1 % is the cleanest of the four
settings tried.

### 4.2 `adaptive_rad` on the same file

Replacing `adaptive_fad` with `adaptive_rad` compiles and runs at about 180x
real time, but over 400 000 samples it does not converge:

- drive drifts down to 12;
- tight to about 50 Hz;
- tone stays near 3000 Hz;
- the residual is 0.056 rms.

This is the documented behaviour: through a recursion the reverse sweep
returns the direct term with the past state held fixed (pseudo-linear
regression), and here all four filters are recursive. The example therefore
argues for the library's rule: `adaptive_fad` for recursive models. The
proposal states it qualitatively in the "Try" paragraph and quotes no
figures, since the test does not check them.

### 4.3 Design decisions recorded in the fixture header

- **`fi.peak_eq_rm`, not `fi.peak_eq`.** The latter takes `abs` of its gain,
  whose derivative at the 0 dB default is not a number (F1).
- **A `sin(2 pi phasor)` envelope, not `os.osc`.** This keeps the file
  runnable under `adaptive_rad` (F2).

## 5. Findings during prototyping

**F1. `fi.peak_eq` is not differentiable at its 0 dB default.** Under
`adaptive_fad` the `mid_gain` gradient is not a number on the first frame,
and Adam then pushes the slider to its bound, +12 dB, where it stays. This is
a modelling wall rather than a compiler defect, and D1 documents it.

Open question: should the clocked loops (`descend_N_*_clocked`) skip a step
whose frame gradient is not finite, instead of clipping to a bound? That
would be a library behaviour change, outside this proposal.

**F2. `rad` refuses the read table of `os.osc`.** The excitation used
`os.osc(1.3)`, a signal that does not depend on the seeds, and the compile
failed with:

```
[FRS-SFIR-0004] signal RdTbl(...) not supported in BlockReverseAD backward pass (B6)
```

`docs/rad-note-en.md` §3.6 says read-only tables are supported, with the
table contents treated as constant. One possible link to check is the runtime
table fill, now the default (`--table-init runtime`): it may no longer
present the oscillator table as a write-once `WrTbl(_, _, nil, nil)`.

To be investigated separately. Minimal reproducer: any `rad` loop whose loss
reads `os.osc(f)`. The examples work around it with `sin(2 pi phasor)`.

**F3. `adaptive_rad` drifts on a recursive model** (§4.2). This is expected
and already documented. It is recorded here as the measured instance the
example quotes.

## 6. Status and next steps

- **Implemented, uncommitted:**
  - D2: fixture `tests/corpus/ddsp_fad_adaptive_pedal.dsp` and test
    `fad_adaptive_pedal_learns_its_six_sliders_without_being_rewritten`;
  - golden-check passes.
- **Pending approval of this proposal:**
  - D1, with its three tests in `crates/cranelift-ffi/tests/tutorial_examples.rs`;
  - D3, D4 and D5.
- **Questions for review:**
  1. Is the placement of the tutorial section right (new §13, renumbering
     §13 and §14)? The alternative is §11.5, next to the clocked loops it
     builds on.
  2. Should the ambitious example also quote measured `adaptive_rad` figures,
     with a second test pinning them, or stay qualitative?
  3. Should F2 be fixed before the examples land, so that the excitation can
     use `os.osc` as the other examples do?
