# `faustprobe`: feedback quality. Analysis and plan

Date: 2026-09-17

## Scope

`faustprobe` (design: `porting/faustprobe-generic-test-tool-design-2026-08-14-en.md`,
guide: `docs/faustprobe-user-guide-en.md`) has become the feedback instrument of
the Faust work done with an agent in the loop: the sibling projects
`faust-diff-ampli`, `-rir`, `-fdn` and `-jot` call it for every measurement, and
three changes made from `faust-diff-jot` on 2026-09-16 and 09-17 each removed a
place where the tool answered without informing (`--set` refused with `--train`;
bargraphs listed like sliders, writable without effect, unreadable; a compile
error reduced to "errors=1, diagnostics=1").

This document asks what remains of that kind, and what would make the feedback
more precise and more attributable. It has two parts: an analysis, whose claims
about the present tool were each run on the release binary of commit `79f2d6af`,
and a plan in five phases. **Status: the five phases are implemented
(2026-09-17, §7).**

The design document already states the principle the analysis applies (its §6):
the tool is a measuring instrument, and an instrument that silently misreports
is worse than none.

## 1. What good feedback is, for this tool

Three properties decide whether the output of a probe run can drive the next
action, of a person or of an agent:

- **local**: tied to a frame, a channel, a control, a source line, a
  sub-expression, rather than to the program as a whole;
- **cheap**: one command, no file to write, no script to adapt, so that a
  hypothesis costs seconds;
- **objectively verifiable**: a number compared with a reference under a stated
  tolerance, and an exit status, rather than a plot to look at.

A fourth property is specific to an instrument: **it says what it did**. A run
that did something other than what the command line asked (clamped a value,
rounded a sample to two digits, rendered silence because a gate was never
pressed) must say so, because its numbers look like measurements either way.

The analysis below sorts the gaps by these properties. Its findings come from
the friction actually met in `faust-diff-jot` (nine rooms fitted, 67 checks,
three effect programs), not from a survey of what probes can do.

## 2. Analysis

### 2.1 Four places where the tool misreports today

Each was reproduced on 2026-09-17; the commands are given so that the fixes can
be checked against them.

**D1. An out-of-range value is clamped in silence.**

```text
$ faustprobe --sweep gain=0.5,1,7,100 --reduce peak --in dc -n 64 demo.dsp    # gain: hslider 0..1
gain,peak_out0
0.5,0.521748543
1,1.043497086
7,1.043497086
100,1.043497086
```

`--set gain=7` likewise runs at `gain = 1` and exits 0. The clamp is deliberate
(`Control::clamp`, `probe/params.rs`: "keeps a mistyped command line from being
silently absurd") and right in intent: a Faust host never writes outside a
widget's range, and a DSP is not compiled to expect it. But the clamp itself is
silent, and a sweep then prints rows labelled 7 and 100 that are measurements of
1. This is the defect fixed for bargraphs on 2026-09-17 (identical rows that
look like a measurement), one step further along the same path.

**D2. "Full precision" is nine fixed decimals.**

The `--format csv` help says "with full precision"; the code prints
`format!("{:.9}")` at three sites of `bin/faustprobe.rs` (the per-frame dump,
the statistics, the sweep rows). Consequences measured:

```text
$ echo 'process = 1.0e-7 / 3.0, 1.0/3.0;' > tiny.dsp; faustprobe --double --in zero -n 1 tiny.dsp
0,0.000000033,0.333333333
```

- in `--double`, seven of sixteen digits are lost on values near 1: the test
  tolerances of `faust-diff-jot` are 1e-7 and 1e-8 for that reason alone, and
  the 2.6e-9 residual between a preset and `jot_reverb.dsp` set to the
  displayed values is the rounding of the printed gain, not a property of the
  programs;
- fixed decimals are worse than they look: a sample of 3.3e-8 keeps **two**
  significant digits, in either width. A reverberation tail below -140 dB, a
  small gradient, a residual: exactly the values one inspects when something is
  subtly wrong. (`--train` rows already print `2.388789544e-4`: the problem was
  met and solved there, locally.)
- the converse defect is visible in `--list-params`, which prints an `f32` step
  through its `f64` conversion: `0.0010000000474974513` for a step of `0.001`.

A second cost is speed: a 4 s render of a 16-output program is 176 400 CSV rows
that Python parses line by line.

**D3. A non-finite render does not say where.**

```text
$ faustprobe --in zero -n 600 nan_at.dsp      # process = (500 - n) : sqrt, n = +(1) ~ _
...
599,NaN
faustprobe: render produced non-finite samples
```

`StatsAccumulator` (`probe/render.rs`) keeps one boolean per channel. The frame
is known when the flag is set and is thrown away. For the instability of
`jot_reverb.dsp` under slider jumps (2026-09-16), what was needed was: the first
non-finite frame, its channel, the control values then, the last `--at` event
before it; and, earlier than the NaN, the frame at which the level started to
run away.

**D4. Silence comes with no hint.**

```text
$ faustprobe -n 4096 --quiet gate.dsp         # os.osc(440) * en.adsr(..., button("gate"))
# out0: peak=0.000000000 rms=0.000000000 dc=0.000000000 finite=yes
```

Exactly-zero output is nearly always one of three facts the tool already knows:
a button or checkbox that gates the sound is at 0; the input is `zero` and the
program has inputs; `--nvoices` is set and no note is scheduled (the `--nvoices`
help text itself warns that this renders silence). None is reported.

### 2.2 Attribution: what has to be written in Python today

**A1. Comparing two renders.** Written by hand about ten times in
`faust-diff-jot`: a refactoring that must not change a sample (library code
shared between `jot_presets.dsp` and `jot_reverb.dsp`), `fad` against `rad`, a
preset against the adjustable program set to its values, the bargraph variant
against the plain one. Each time: two renders, two CSV parses, a maximum of the
difference. The maximum is also the least informative answer: **the first frame
that differs** says whether two programs diverge at the onset, at a control
event, or slowly.

**A2. Invariants the guide states and the tool does not check.** The user guide
says that a result which moves with `--block` is itself a finding. Nothing runs
that check. The same holds for three more properties that are cheap and
objective:

- *reset*: a render after `instanceClear` equals the first render. Sweeps and
  `--reset-per-block` rely on it: if a clear left state behind, every sweep point
  after the first would be contaminated, silently;
- *width*: the distance between the `f32` and the `f64` render. A large one marks
  a numerically fragile program (a long accumulation, a near-cancellation);
- *determinism*: two fresh factories give the same bytes (the design document
  lists it as a validation of the tool, §6.4; it is not exposed).

A useful fact for `block`: a `rad` program is a genuine positive. On
`tests/corpus/ddsp_rad_host_block_resonator.dsp` the loss lane is identical at
`--block 64` and `256`, and the gradient lanes differ at frame 100
(1.294441594 against 1.294328502), as the block reverse sweep implies. So the
check has a fixture that must fail on lanes 1-2 and pass on lane 0.

**A3. Asking a question about a sub-expression costs a file.** To read
`absorb_pole_exact(1709, 2.0, 0.5)` out of `jot.lib` one writes a `.dsp` with an
`import` and a `process`, as was done again while preparing this analysis;
`jot_coefs.dsp` (150 outputs) and `jot_matrix.dsp` (784) exist for that purpose
only, and a `.lib`, having no `process`, cannot be given to the tool at all. The
mechanism to do better is already in the probe: the polyphonic engine extracts
`effect` by wrapping the source in `environment{ ... }` (`probe/engine.rs`,
`compile_from_string`). Evaluating an arbitrary expression in a file's scope is
the same wrap.

### 2.3 The host loop

`--train` prints the loss and the controls. What was learned the hard way in the
room fits:

- a control that ends **on a bound** is not converged, it is stopped: the
  studio's high-frequency time ran into the lower bound of its control and this
  was read off a table afterwards. The optimiser's projection onto the range is
  part of the algorithm and stays; its having been active is information;
- no gradient magnitude is shown, so a flat landscape (the slope-based room
  loss, one of the four rejected before the variance loss) and a vanishing step
  look the same;
- `--fd-check` runs at the starting point only; at the end point it would
  confirm that the descent stopped where the gradient is small *and* right;
- grid-then-descent, which the non-convex toolbox recommends and
  `scripts/fit_rooms.py` implements as a `--sweep` run, a parse, then a `--set`
  + `--train` run, is refused as one command (`--sweep` does not combine with
  `--train`).

### 2.4 Cost

`fad` against `rad` was timed by hand (2.0 s against 5.3 s for 50 blocks). The
tool knows its compile time and its render time and reports neither, so a
compiler change that doubles the cost of a program is invisible in every run
that would have shown it. Subnormal samples are a related blind spot: a
reverberation tail produces them, they cost CPU on some targets, and the
statistics count nothing.

### 2.5 The typed channel

Since 2026-09-17 a compile failure prints the compiler's human text. The
compiler also has a typed channel (diagnostics-v2 JSON: code, range, a
machine-applicable fix), which `faust-rs --check --error-format json` and the
`wasm-ffi` bindings expose and the probe does not. For an agent, the JSON is the
better input: a fix can be applied without reading prose.

### 2.6 What should stay out

Band filters, Schroeder integration, T30, modal analysis: no. §9 of the user
guide draws the line (a `--reduce` returns a scalar; vectors belong to an
analysis script) and the line is right for a second reason the guide does not
give: **independence**. The numpy reference of each sibling project has value
because it shares nothing with the tool under it. A `faustprobe --reduce t30`
checked by a test that calls `faustprobe` compares the tool with itself. The
plan below adds observability, localisation and honesty; it adds no
domain-specific analysis.

One exception is argued by the design document itself (§7.1, "prefer the impulse
response where the system is linear"): the frequency response of a linear
program, from one impulse response, is domain-neutral, is the first question
asked of any filter, and today costs a sine sweep. It is the last phase, and it
comes with the linearity check that §7.1 asks for ("the tool should not pretend
otherwise").

## 3. Plan

Five phases, ordered by value over cost. F1 is corrective: it changes what the
tool says about what it already does. F2 to F5 add capability. Each phase lands
with its tests, at least one hand-applied mutation per behaviour that the tests
must reject, the user guide, and a journal entry; the `faustprobe` skill
(`~/.claude/skills/faustprobe/`) is updated when a rule of use changes.

| Phase | Content | Exit criterion |
|---|---|---|
| **F1** | D1-D4: range errors, round-trip numbers and `--out`, non-finite and runaway localisation, silence notes | the four reproductions of §2.1 give the outputs of §3.1; `.ir` output byte-identical to before |
| **F2** | `--compare` / `--ref`, `--check block,reset,width,determinism` | the `rad` fixture fails `--check block` on lanes 1-2 only, with the first differing frame; a refactoring check of `faust-diff-jot` is one command |
| **F3** | `--eval EXPR` | every value of `jot_coefs.dsp` is obtainable from `jot.lib` without a file; diagnostics keep the file's line numbers |
| **F4** | host-loop feedback, `--time`, subnormal count, `--error-format json` | the studio fit reports its bound; grid-then-descent is one command |
| **F5** | `--freqresp` with its linearity check | a one-pole and the TPT ladder of the design's §6.2 match their closed forms; a saturator is refused |

### 3.1 F1: the tool says what it did

**D1, range.** A value outside `[min, max]` of its control is an **error**, found
before any render by the validation pass that `check_writable` opened for
bargraphs (`--set`, every `--sweep` value, every `--at` value, a `--set` on a
trained control):

```text
faustprobe: `gain`=7 is outside the range [0, 1] of /demo/gain
```

`--clamp` restores the clamp for the rare intended use and makes it visible:
`# clamped /demo/gain: 7 -> 1` with the statistics, a `clamped` array per JSON
run, and sweep rows that carry the applied value. `Probe::set` keeps clamping
for library callers; the polyphonic engine's unclamped `set_exact` is untouched.
The projection step of `--train` is not an error (see F4).

*Checks.* Exit status and message for the three flags; `--clamp` notice in text
and JSON; a value on a bound is accepted. *Mutations:* validation skipped for
`--sweep` only; notice dropped under `--clamp`.

**D2, numbers.** Every number the tool prints becomes the shortest decimal
string that parses back to the same float **in the width of the factory**
(formatted from the `f32` in single precision, which also fixes `--list-params`),
plain or scientific, whichever is shorter (Rust's `{:?}` for floats has this
behaviour). `--precision N` gives `N` fixed decimals; `--precision 9` reproduces
today's bytes and is the escape hatch for anything pinned to them. `--format ir`
is not touched: its 6-decimal, zero-clamped text is a regression format.

`--out FILE` writes the rendered window and keeps only the statistics on stdout:
`.npy` (shape `(frames, outputs)`, little-endian, `<f8` or `<f4` by width) and
`.wav` (float 32 or 64, which `--in file:` already reads with any channel
count); `.f64`/`.f32` for a single output, the raw layout `--in file:` reads.
Refused with `--sweep`, `--train` and `--format ir`.

*Checks.* A program whose outputs are known constants (`1.0/3.0`, `1.0e-7/3.0`,
`ma.PI`), rendered in both widths: the parsed text is bit-equal to the value
computed in the test. `--precision 9` against lines captured from the current
binary. `--out x.wav` fed back through `--in file:x.wav` into `process = _`
returns the same samples: the reader is existing, independent code. The
impulse-test regression against `impulse_cranelift` stays byte-identical.
*Mutations:* `{:.9}` restored at one site; `f32` formatted through `f64`.

**D3, non-finite and runaway.** The accumulator records the first non-finite
sample per channel. The error becomes:

```text
faustprobe: render produced non-finite samples
  first: frame 23817, out0 (NaN); 64383 of 88200 frames affected
  controls then: /jot_reverb/T60_at_dc=10 /jot_reverb/T60_at_half_the_sample_rate=0.1 ...
  last event before it: frame 22050, /jot_reverb/T60_at_dc=10
```

The statistics line gains `peak_at=FRAME`, appended after `finite=` so that
key-based parsers are unaffected. `--fail-above LEVEL` fails a render whose
magnitude exceeds `LEVEL` and reports the first such frame and channel: the
runaway of a feedback loop is caught thousands of frames before the NaN, and the
flag turns "stays bounded under slider jumps" into an exit status.

*Checks.* `nan_at.dsp` of §2.1: first frame 500 exactly, from the definition of
the program. An `--at` event before the failure is named. `--fail-above` on a
ramp of known slope. *Mutations:* last frame reported instead of first; events
after the failure listed.

**D4, silence.** When every output is exactly zero over the window, facts (not
guesses) are appended as `# note:` lines and as a `notes` array in JSON:

```text
# note: every output is exactly zero over the window
# note: buttons and checkboxes at 0: /gate/gate
# note: input is `zero` and the program has 1 input
```

Exit status stays 0: silence can be the right answer.

*Checks.* The gate program; a program with inputs under `--in zero`; a sounding
program prints no note. *Mutation:* a threshold in place of exact zero.

### 3.2 F2: comparison and invariants

**`--compare OTHER.dsp`** compiles a second program in the same process and
renders both under the same excitation, schedule and window. `--set` applies to
both and must resolve in both (trailing-fragment matching makes
`--set T60_at_dc=2` address `/jot_presets/...` and `/jot_reverb/...` alike);
`--set-a` and `--set-b` address one side. **`--ref FILE`** compares with a render
saved by `--out`. Output, per channel:

```text
# compare out0: max_abs=2.6e-9 at frame 1312, max_rel=3.1e-9, first frame beyond tolerance: none
# compare out1: max_abs=0 (identical)
```

`--tolerance ABS` (default 0: bit equality) and `--rel-tolerance REL` (relative
to the reference's peak); exit 1 beyond them. An arity mismatch is an error.

**`--check LIST`**, with `LIST` among `block=N1,N2,...`, `reset`, `width`,
`determinism`, or `all`:

- `block`: a fresh instance per block size, each compared with the first; reports
  per channel the first differing frame. Expected: identical. Documented
  exception: the gradient lanes of a `rad` program, which are defined per block;
- `reset`: render, `instanceClear`, render again on the same instance; identical;
- `width`: `f32` against `f64`; a report, and a gate only under `--tolerance`;
- `determinism`: two fresh factories, identical bytes.

*Checks.* `_ * 0.5` against `_ / 2`: identical. A program that departs at a known
frame (`select2(n >= 100, x, x * 1.0001)`): first frame 100. The `rad` fixture of
§2.2 for `block`: lane 0 passes, lanes 1-2 fail at the first frame where they
differ. `width` on `+(0.1) ~ _` against the two accumulations replayed in the
test. *Mutations:* for `reset`, a Faust fixture cannot be built (it would take a
compiler bug), so the rejecting mutation is in the probe: the clear is skipped
and the check must fail; for `compare`, the maximum taken over the first channel
only.

### 3.3 F3: `--eval`

```bash
faustprobe --double -I ../faust-rs/libraries -n 1 --in zero \
    --eval 'absorb_pole_exact(1709, 2.0, 0.5)' --eval 'absorb_pole(1709, 2.0, 0.5)' dsp/jot.lib
# frame,absorb_pole_exact(1709, 2.0, 0.5),absorb_pole(1709, 2.0, 0.5)
# 0,0.19811648...,0.50192830...
```

The file is wrapped as the polyphonic engine wraps an instrument:
`__probe = environment{ <source>  __eval0 = EXPR0; ... }; process = __probe.__eval0, ...;`
so each expression is evaluated **in the file's own scope** (a library's
functions unprefixed, as inside the library), with its imports. A file's own
`process` is ignored, so `.lib` and `.dsp` are treated alike. Everything else
(`--set`, `--sweep`, `--list-params`, `--compare`) applies to the resulting
program unchanged. The expressions head the CSV columns.

Two details decide whether the feature is usable: the opener goes on the file's
first line, not before it, so that **diagnostics keep the file's line numbers**;
and an error inside an expression is reported against `<eval 0>`, with the
expression as its source line.

*Checks.* Each `--eval` value equals the output of the hand-written program that
states the same expression (the independent route is the one used today). A
fixture with an error on its line 3 reports `:3:`. An undefined symbol in the
expression reports `<eval 0>`. *Mutations:* the opener placed on its own line;
two expressions emitted in the wrong order.

Expected effect in the sibling projects: `jot_coefs.dsp` and `jot_matrix.dsp`
remain as the versioned statement of what is checked, but exploring a library
stops costing a file, and a `.lib` can be compile-checked directly
(`--eval '0'`).

### 3.4 F4: host loop, cost, typed errors

- **Bounds.** The closing lines of `--train` say which controls ended on a bound
  and for how many blocks: `# trained /x/ltpi=0.030000000 (on its lower bound for
  212 of 250 blocks)`. JSON alike.
- **Gradients.** `--train-verbose` adds the mean gradient per control to each
  row; the closing lines give the minimum loss and its block, and flag a final
  loss more than ten times above it.
- **`--fd-check` at the end**: `--fd-check end` (or `both`) runs it at the
  trained values.
- **Grid then descent.** `--sweep` is accepted with `--train` when every swept
  control is trained: the loss of one block is evaluated at each grid point from
  a cleared instance, the descent starts from the best, and the grid is printed.
  Replaces the two-call pattern of `fit_rooms.py`.
- **`--time`** prints compile time, render time, the real-time factor and the
  worst block against its budget. Behind a flag, never in default output:
  determinism of the output is a validation criterion of the tool.
- **Subnormals.** `subnormal=N` in the statistics when `N > 0`, for the outputs
  (internal ones are not observable; the guide must say so).
- **`--error-format json`**: on a compile failure, the diagnostics-v2 document on
  stdout, as `faust-rs` prints it. The bundle is reached at the Rust level,
  inside `cranelift-ffi`, next to the per-thread complete text; whether the C
  ABI should also expose the JSON is an open question (§6).

### 3.5 F5: `--freqresp`

`--freqresp N[:FMIN:FMAX]` renders the impulse response (`-n` frames, `--skip`
ignored) and evaluates its transform at `N` log-spaced frequencies by direct
summation, so that no bin grid constrains them: rows `hz,mag_db_out0,phase_out0`.
Before that, it renders the impulse at half amplitude and requires the two
responses to be proportional within a tolerance: a program that fails is
refused, with the frame at which proportionality breaks. The truncation of the
response is reported (energy of the last tenth of the window), since a response
still ringing at `-n` gives a wrong magnitude.

*Checks.* The closed forms of the design's §6.2 (one-pole, TPT ladder at
resonance 0). `ef.cubicnl` is refused. *Mutation:* the linearity check skipped.

## 4. Compatibility

Two defaults change in F1, both deliberately:

| Change | Who is affected | Mitigation |
|---|---|---|
| out-of-range value is an error | a command that relied on the clamp | `--clamp`; the message names the range |
| numbers are round-trip, not `%.9f` | a consumer comparing text, a golden file | `--precision 9`; parsers using `float()` are unaffected |

`--format ir`, the impulse-test protocol and the JSON `schema_version` rule are
unchanged: keys are added (`clamped`, `notes`, `compare`, `timing`), none is
renamed, and added keys do not bump the version, as for `bargraphs`.

**Qualification gate of F1**, since it changes defaults: the whole
`cranelift-ffi` suite; the impulse-test byte identity; the outputs quoted in
`docs/faustprobe-user-guide-en.md` re-run and updated; and the sibling suites
with the new binary (`faust-diff-jot` `make test`, 67 checks; `faust-diff-fdn`;
`faust-diff-rir`; `faust-diff-ampli`), whose tolerances can then be tightened
from 1e-7 to what the programs actually achieve, which is itself a result.

## 5. Non-goals

- Domain analysis (§2.6): octave bands, decay times, modal fits.
- Internal signal taps by name. `--eval` answers most of the need; a true tap
  needs compiler support (naming a node of the signal graph) and belongs to a
  separate design.
- Audio playback, plotting, a watch mode.
- An interpreter backend for cross-checking (an open question of the design
  document, still open, and orthogonal to this plan; `--compare` would serve it
  the day it exists).

## 6. Open questions

- **Error by default, or notice by default, for D1?** The plan says error: a
  host cannot produce the state, so a run that needs it is almost always a typo.
  The alternative (clamp and print the notice) is friendlier to old command
  lines and keeps a wrong run looking successful to anything that reads only
  the exit status.
- **`--check block` and `rad`.** The tool cannot know which lanes are gradients.
  Either the exception is documentation only, or `--check block` takes the
  lanes to check (`block=1,64:lanes=0`).
- **JSON diagnostics through the C ABI.** A sixth `getCComplete*` returning
  diagnostics-v2 would give every C host the typed channel; it also freezes that
  schema into the ABI.
- **`--eval` and `declare`/`process` clashes.** A file that declares options
  (`declare options "[nvoices:8]"`) inside an `environment` needs checking; so
  does a library that itself defines `process`.
- **Round-trip text and `f32`.** Whether the statistics (computed in `f64` from
  `f32` samples) are printed as `f64` (they are sums) while samples are printed
  as `f32`. The plan assumes so; it should be stated in the guide.

## 7. Status

### F1, implemented 2026-09-17

As planned, with these decisions and departures:

- **D1 is an error by default** (first open question of §6), with `--clamp` as
  planned. The error names the query, the value, the range and the resolved
  path, and points at `--clamp`. Under `--clamp` a sweep's rows and the JSON
  `set` carry the applied value. With `--train`, every `--set` is validated the
  same way before the descent. **Not applied to `--nvoices`**: the polyphonic
  wrapper writes a voice's controls unclamped on purpose, as `poly-dsp.h` does
  (a synthesized frequency must reach the zone as computed), so its `--set` keeps
  that rule and `--clamp` is refused there.
- **D2.** Round-trip text is Rust's `{:?}` of the float at the program's width;
  a peak, a bargraph and a control's bounds are at that width, a mean, an RMS,
  a reduction and a trained control are `f64` (last open question of §6, as
  assumed). A training loss is `{:e}`. `--precision 9` was compared with the
  previous binary by hash on a dump, a windowed and thinned dump, a bargraph
  dump, a two-axis sweep and a training run: identical bytes. The statistics
  line is the one text that is not, by its appended `peak_at=`.
- **`--out`** streams `.npy`, float `.wav` and single-output `.f64`/`.f32`, and
  refuses `--every` as well as the planned `--sweep`, `--train`, `--format ir`
  (every frame is written; a flag that would be ignored is refused), and
  `--nvoices`.
- **D3.** The context of a failure lists the controls the command line had
  *written* by the failing frame with their values then, not every control: the
  others are at their initial values by definition, and a GRU has 27. When both
  happen, `--fail-above` is reported first if its frame comes first, with the
  non-finite frame mentioned under it: the runaway is the cause, the overflow
  its consequence. `--precision`, `--out` and `--fail-above` are refused by the
  impulse-test protocol rather than ignored by it.
- **D4** as planned, plus the polyphonic note (no `--note`/`--chord` scheduled).

Checks: `tests/feedback_probe.rs` (20 tests, library and binary; every expected
frame, value and count follows from the fixture's definition, including a
hand-computed two-step descent) and unit tests in `probe/render.rs`,
`probe/number.rs`, `probe/audio_out.rs` (the `.wav` and raw files are read back
by the existing, independent `audio_file` reader; the `.npy` is decoded from the
format's specification). Thirteen hand-applied mutations, each rejected: sweep
values not validated; a clamp not recorded; a sweep row carrying the requested
value; the dump back to `{:.9}`; an `f32` printed through its `f64`; the last
non-finite frame instead of the first; writes scheduled after the failure
listed; silence as a threshold; the level checked before the window; an overflow
hiding the runaway; the `.npy` shape transposed; a training run not validating
its `--set`; the polyphonic note dropped.

Qualification gate (§4): the `cranelift-ffi` suite, 248 tests; `--protocol
impulse-test` byte-identical to `impulse-cranelift` on the 133 programs of
`tests/impulse-tests/dsp`, all non-trivial; the guide's quoted outputs re-run
(the bargraph example had been illustrative and is now a real run, the
first-contact example names its program; the two whose fixtures are not in the
repository keep their text through `--precision 9`, the flag that produces it); `faust-diff-jot` `make test` 67/67, `faust-diff-fdn` `make
probe-check`, a `faust-diff-ampli` sweep script, all unchanged in their
conclusions and none relying on a silent clamp.

One prediction of §2.1 was measured: with round-trip numbers the residuals of
`faust-diff-jot` that the analysis attributed to printing are gone, bargraphs
against the fitted record 4.3e-10 -> 0 and `jot_reverb.dsp` against a preset
2.6e-9 -> 6.2e-16.

### F3, implemented 2026-09-17

`--eval EXPR`, repeatable, as planned (`probe/eval.rs`, `EvalProgram`). What
the plan left open, and what was found:

- **Layout.** The opener shares the file's first line, and each expression is
  alone on its own line, its definition's name on the line before and its `;`
  on the line after: a diagnostic about an expression then has exact columns and
  the expression as its source line. `EvalProgram::explain` renames such a
  location `<eval k>:1:COL` (the terminator's line, where a parser reports what
  an expression left open, becomes the expression's end) and adds a note naming
  the expression. It is text surgery on the rendered diagnostic, and is written
  so that a text it does not recognise is left as it is.
- **Several outputs.** With more than one expression the columns are attributed
  by arity, which a second program computes (`process = outputs(E0),
  outputs(E1), ...`, one frame). Assuming one output each would have been wrong
  in silence the day an expression has two; `labels` refuses arities that do
  not add up to the program's outputs.
- **The source's name is the file's path**, not a bare name: it is what
  diagnostics cite, what the control paths' root comes from (`/jot_ir/exact`,
  as when the file is compiled), and what the compiler resolves the file's
  relative imports against. A first version also added the file's directory to
  the import path; a mutation that removed it survived, which is how the
  redundancy was found, and the code was dropped for a mutation on the name,
  which is rejected.
- **Scope.** An expression sees the file's top-level definitions. One local to
  a `with` block is not visible, and the diagnostic then often cannot locate the
  use (the name also occurs in the file); `explain` attributes an unlocated
  undefined symbol to the expressions that use it and states the rule.
- **Open question of §6** (`declare` and `process` inside the environment):
  both are accepted; a test wraps a program with `declare options` and a
  `process` of another arity.
- CSV header fields holding a comma are quoted; a `# eval outN = EXPR` legend
  goes with the statistics and before a sweep's rows; JSON gets an `eval` array.
  Refused with `--nvoices` and the impulse-test protocol.

Exit criterion. The 150 outputs of `faust-diff-jot/dsp/jot_coefs.dsp` were
obtained from `jot.lib` alone with thirteen `--eval`: 126 are identical text for
text, 24 differ by one to three units in the last place (relative 1e-16 to
3e-15). The difference is not the wrapper's: `jot_coefs.dsp` computes from two
sliders at run time and the expressions had literal arguments, folded at compile
time; with sliders as arguments `--eval` gives the file's digits
(`0.2840050746078115` against `0.28400507460781155` for `absorb_pole(967, 2.0,
0.5)`). Diagnostics keep the file's line numbers (`broken.lib:3:15`).

Checks: `tests/eval_probe.rs`, 10 tests (an expression against the hand-written
program that states it, labels and arities, a program with its own `process`,
controls, sweeps and listing, relative imports from another directory, errors
in the file and in an expression, the `with` rule, a training run whose loss is
an expression, JSON, refusals) and 6 unit tests. Six mutations rejected: the
opener on its own line; the expressions in reverse order; arities from
`inputs`; the source named by a bare name; a location not renamed; a header
field not quoted. A seventh survived, the file's directory removed from the
import path, and showed that code to be redundant (above).

A correction to §3.3, which says that a `.lib` can be compile-checked with
`--eval '0'`: it can be *parsed* that way. Faust evaluates lazily, so only what
an expression uses is evaluated; a library with an undefined symbol in one of
its functions passes `--eval 0` and fails, at the library's own line, when that
function is evaluated. The guide says so and a test holds both halves. The
first write-up of this phase had the wrong claim, and a `make check` line added
to `faust-diff-jot` on its strength was withdrawn: the programs that import the
library already parse it.

Found on the way: `process = 2.0 / 0;` panicked the compiler (and aborted a host
through the FFI) instead of giving a diagnostic. Fixed the same day: a constant
division by zero is `FRS-EVAL-0007`, as the reference compiler's `ERROR :
division by 0 in 2 / 0` (journal, 2026-09-17).

### F2, implemented 2026-09-17

`--compare OTHER`, `--ref FILE`, `--check`, as planned (`probe/compare.rs`,
FFI-free like `probe/render.rs`). Decisions and findings:

- **What a comparison reports.** Per output: bit identity; the largest distance
  and its frame; that distance relative to the reference's peak; and the first
  pair beyond the tolerance, with both values. The tolerance is `abs + rel *
  peak(reference)`, zero by default, and zero means bit equality. A non-finite
  sample agrees only with the same bits (a NaN distance is beyond any
  tolerance, which `distance > limit` alone would let through).
- **The verdict comes after the output.** A failed comparison prints its lines,
  or its JSON document, then fails: the details are what one reads, the exit
  status what a gate reads. The error reuses the context of a failed render
  (F1): the controls written by that frame and the last `--at` before it, which
  is what names the event two programs answer differently.
- **`--check block`** compares other sizes with the render's own `--block`
  rather than with the first of the list, so the check is about the render one
  is looking at. Open question of §6 settled with `--compare-outputs`, general
  to comparisons and checks: on the `rad` fixture the loss lane is identical at
  `--block 256` and the gradient lanes differ from frames 1 and 2.
- **`--check determinism`** compiles again and says whether the program key is
  the same. The key is a digest of the canonical FIR, so a second compilation
  with the same key shares the cached factory and the comparison is then a
  formality; a different key is a non-deterministic compilation, and the render
  says whether it matters. It always demands the very bits.
- **`--check width`** is a report unless a tolerance is given: the two widths
  never agree to the bit, and a gate that always fails teaches nothing. Its test
  replays the two accumulations of `+(0.1) ~ _` and finds the printed `max_abs`
  to the bit (3.0e-3 after 2000 frames).
- **`--ref`** needed a `.npy` reader, which `--in file:` gains too
  (`probe/audio_file.rs`: formats 1.0 to 3.0, `<f8`/`<f4`, C order, one or two
  dimensions, anything else refused by name). A file must hold the same window.
- `--set` goes to both programs and must resolve in both; `--set-a`/`--set-b`
  to one. The second program's writes are validated like the first's.

Exit criterion. One command does the comparisons `faust-diff-jot` writes in
Python: the bargraph variant against `jot_presets.dsp` (`identical`), and a
preset against `jot_reverb.dsp` set to its displayed values (9e-17 of the peak,
first differing frame 1516, inside `--rel-tolerance 1e-12`). `--check all` on
`jot_reverb.dsp` takes 2.7 s and passes; its `width` report is 4.8e-9.

Found on the way, fixed in its own commit: a regression of F1. A value typed on
a decimal bound (`--set x=0.7`, maximum 0.7) was refused, because a control's
bounds reach a host in single precision and the command line's value is a
double. F1's test of the bounds used 0 and 1.

Checks: `tests/compare_probe.rs` (13 tests), 7 unit tests of the comparison, 3
of the `.npy` reader. Nine mutations rejected: only the first output compared;
the last disagreement instead of the first; the relative tolerance ignored; a
render for comparison not starting from a reset instance (the plan's mutation
for `reset`, which has no Faust fixture); the block check at the same size; the
second program given the first's own values; a failed comparison not failing the
command; the width check never gating; a `.npy` read column-major, which a first
version of the two-output test did not see (its outputs were constant and
equal) and the rewritten one does.

### F4, implemented 2026-09-17

As planned, with these decisions and departures:

- **Bounds.** A block counts when its step leaves the control on the bound
  (`<=` / `>=` after the projection). The `# trained` line says for how many
  blocks and whether the last one is among them (`on its lower bound for 84 of
  300 blocks, the last one included`, or `..., not the last one` for a control
  that met a bound and left it); a control that never did is printed as before.
  One `# note:` names the controls that *end* on a bound: stopped, not
  converged.
- **The best loss.** `# loss: minimum M at block B`, the first block that
  reached it. The ×10 flag is a `# note:` and carries **the controls that
  block ran with** (`values_at_min`: those before its step), which is what one
  wants from a descent that left its minimum. It is a ratio, so it is said for
  a positive minimum only. The old `# loss: block 1 ..., block N ...` line is
  unchanged.
- **`--train-verbose`** adds `grad_CONTROL` columns: the block's mean gradient
  at the controls the block ran with, not the step. The JSON rows always carry
  `grads`.
- **`--fd-check[=start|end|both]`.** The value needs its `=` (clap
  `require_equals`), so that `--fd-check FILE` remains the bare flag followed
  by the program. `end` needs a descent (`--blocks 0` is refused). Its failure
  comes after the rows and the trained values, as F2's verdicts do.
- **The reference of `--fd-check` was the finding of this phase.** Run at the
  end of the corpus resonator's descent, the check printed `rad 1.2e-11 fd
  1.7e-2`, a hair under the tolerance, for a gradient that is right. A central
  difference is off by `f''' h^2 / 6`, which does not shrink with the
  gradient; scanning `--fd-step` over 1e-2, 1e-3, 1e-4 gave relative errors of
  3.98e-4, 3.98e-6, 3.98e-8, exactly `h^2`: the number this check had always
  printed was the finite difference's error, not the gradient's. The reference
  is now extrapolated from the steps `h` and `h/2` (Richardson, `(4 D(h/2) -
  D(h)) / 3`, two more evaluations per control): the same start check reads
  `3.27e-13`, the end check `8.8e-8`, and rounding takes over below `--fd-step
  1e-4`. The JSON keeps the plain difference as `fd_plain`. This changes the
  numbers `--fd-check` prints (they shrink); its line format, which
  `faust-diff-jot` parses, does not change.
- **Grid, then descent.** `--sweep` with `--train`, every swept control a
  trained one. One instance, reset per point, as the descent does under
  `--reset-per-block`: the descent's first block is then the grid's block at
  the best point, the very same number, which the test asserts. Points are `#
  grid PATH=V ... loss=L` lines (stdout keeps one CSV table), the best tagged;
  a non-finite point is listed and never chosen; a tie keeps the first; swept
  values are range-checked like any write; on a control given `--set` and
  `--sweep` the grid decides. `--fd-check` at the start runs at the best point.
- **`--format json` with `--train`** did not exist (the flag was ignored, which
  is a case of §2.1). It prints one document, at the end or with the failure
  that ended the run: a failed `--fd-check`, or a loss that is not finite, whose
  error now names the block **and its controls**. `--format ir` is refused.
- **A starting point typed on a decimal bound** (`--set x=0.7`, maximum 0.7)
  was clamped in `f64` against the `f32` bound and left from 0.699999988: the
  same family as F2's regression, in the host loop. It uses the control's own
  clamp now.
- **`--time`.** `compute` alone is timed (`probe/timing.rs`, FFI-free), behind
  `RenderSpec::time` so that an untimed render reads no clock. The worst block
  is the worst **against its own budget**: a block cut by an `--at`, or the
  last one, has a shorter deadline. A sweep's rows and an `.ir` text get one
  account on stderr; the `.ir` text is untouched, so the flag is accepted under
  the impulse-test protocol. `--train` and `--nvoices` are timed too; a
  descent names its worst block by number. What it showed at once: the debug
  build of the tool compiles 25 times slower than the release one (5.65 s
  against 230 ms for 400 one-poles) and computes at the same speed.
- **Subnormals** are counted **at the program's width** (an `f32` subnormal
  is a normal `f64`, and the statistics are accumulated in `f64`), over the
  window, with the frame of the first: `subnormal=23 subnormal_at=127`, only
  when there is one; always present in the JSON channels. Not in the
  polyphonic statistics, which have their own, smaller, accumulator.
- **`--error-format json`.** The report is reached at the Rust level
  (`cranelift_ffi::factory::last_error_diagnostics_json`, per thread, attached
  where a typed error is flattened and published where its summary reaches the
  buffer, so that an untyped failure publishes none) and recorded by the probe
  with the text of the failure (`engine::last_compile_failure`). The binary
  prints it only when the error it ends on *contains* that text: the
  polyphonic wrapper recovers from a failed `effect` extraction, and the run
  may end on something else. It is the **complete** report (the field set the
  crate documents for FFI consumers and the WebAssembly bindings return), a
  superset of what `faust-rs --error-format json` prints: a departure from the
  plan's "as `faust-rs` prints it". stderr keeps the summary line. **Refused
  with `--eval`**: the ranges are byte offsets in the wrapped source, and a fix
  applied to the file at those offsets would land elsewhere; the human text is
  rewritten for that case, the report is not. The open question of §6 stays
  open: the C ABI does not export the report.

Exit criterion. The studio fit of `faust-diff-jot` as one command (`--sweep
lt0=... --sweep ltpi=... --train lt0,ltpi --reset-per-block --fd-check=end
--time`, 25 grid points and 300 passes over 77 202 frames, 17 s): `ltpi=-3.5
(on its lower bound for 84 of 300 blocks, the last one included)`, minimum at
block 248, gradients right at the end to 5.3e-6, 34 times real time. It
replaces `grid_start` and `fit` of `scripts/fit_rooms.py`, a sweep, a parse and
a second command; the script was left as it is.

Gates: the crate's 329 tests; `--protocol impulse-test` byte-identical to
`impulse-cranelift` on the 133 corpus programs; `faust-diff-jot` `make test`
(67/67) and `make invariants` with the new binary.

Checks: `tests/host_loop_probe.rs` (18 tests, on fixtures that carry a
hand-written gradient lane: the host loop needs a loss and its gradient, not
`rad`, and a closed form lets every expected number be replayed),
`tests/cost_probe.rs` (9), six more in `tests/compile_error_probe.rs`, 5 unit
tests of the timing arithmetic, 1 of the subnormal count. Twenty-three
mutations rejected: `ends_on` never cleared; the controls of the minimum taken
after the step; the gradient reported as the step; the grid keeping the highest
loss; the descent not starting from the best; the grid not writing `--set`;
the first axis fastest; a tie keeping the last point; `--fd-check=end` at the
starting values; the plain difference as the reference; the starting point
clamped in `f64`; a bound counted on the value before the step; any last loss
above the minimum flagged; a block's budget that of `--block`; the clock
started after `compute`; the worst block the longest one; `--time` on by
default; the polyphonic render untimed; subnormals measured as `f64`;
subnormals counted before the window; any error after a compile failure given
its report; an untyped failure leaving the previous report published; stderr
keeping the rendered text under `--error-format json`. One of them (the bound
counted before the step) was first written as a change that changed nothing
and survived for that reason; rewritten, it is rejected.

Not done, and known: the polyphonic path still clamps a `--set` in silence
(F1 refused `--clamp` there instead of reporting).

*Later the same day*, `scripts/fit_rooms.py` of `faust-diff-jot` was moved to
the one-command form (its commits `b1651c8`, `38fd801`): on the nine rooms the
trajectories are those of the two commands bit for bit, 300 rows each. Running
it was the first `make rooms` since F1, and F1's range error stopped it on the
first room: the script renders each fitted model with another program whose
slider went from 0.1 to 10 s, the studio's fitted time is 0.035 s, and until F1
that value was clamped in silence. The published row of that room had been
measured on a model at 0.1 s (T30 error 59 / 134 ms, gain 26.3 dB) while its
preset used 0.035 s; measured on the fitted model it is 94 / 169 ms and
38.4 dB. The case §2.1 opened this plan with, found in the project the plan
was written from.

### F5, implemented 2026-09-17

`probe/freqresp.rs`, FFI-free, and `run_freqresp` in the binary. As planned,
with these decisions and departures:

- **Three checks, not one.** The plan asked for the impulse at half amplitude
  and proportional responses. That is homogeneity, and "linear" stands for
  more: a tremolo or an LFO-modulated filter is perfectly homogeneous and has
  no frequency response, and so is a median filter, whose response to one
  impulse is zero at any level. Three renders, each against what the unit
  response `h` predicts: an impulse of **-0.5** (`-0.5 h`; the negative factor
  is for a rectifier, which doubles when its input doubles), the impulse **37
  frames later** (`h` delayed; 37 divides no block size), and **1 at frame 0
  with 0.5 at frame 1** (`h[n] + 0.5 h[n-1]`; adjacent, because a rank-order
  filter looks at a short window). The first failure names the property, the
  first frame, the two values and the usual cause.
- **To the bit.** The factors are powers of two, so in a linear program the
  first two checks introduce no rounding of their own, and what they print is a
  measurement: `0.0` for a Butterworth, `6.1e-18` for `re.zita_rev1_stereo`,
  which turned out to output `2e-20` with no input at all (its guard against
  subnormals). Superposition holds to rounding; the tolerance is relative to
  the expected response's peak, `1e-9` in double and `1e-4` in single by
  default, `--linearity-tolerance` otherwise. Necessary conditions at the
  amplitudes of one excitation: the guide says so.
- **`--settle N`, not planned, and needed at once.** The time-invariance check
  refuses every program that smooths its sliders (`dm.zita_light`, most
  `*_demo`): after a reset the smoothed gain is an envelope. That refusal is
  right, a response taken during the ramp being that of no filter, and useless
  without a way out: `--settle N` renders `N` frames of silence, puts the
  impulse on frame `N` and counts the response from there. The error suggests
  it.
- **The render of silence** is made only to explain a refusal, and blamed only
  when it is of a size to explain it (above the tolerance times the response's
  peak): an offset of 0.25 is, a reverberator's `8e-21` is not.
- **`--skip` is refused**, not ignored as the plan had it: an option that is
  accepted and does nothing is what F1 removed. So are `--sweep` (one response
  per command; a shell loop does a family of curves), `--at` (a control change
  during the response makes the program time-varying), and what looks at the
  frames of a plain render. `--in` must be `impulse` or `impulse:CH`; a program
  without inputs is refused.
- **The transform** is Horner's rule on the polynomial in `z^-1`, from the last
  sample: a product by a constant of modulus one per sample, the small samples
  of the tail summed first. Interior frequencies are rounded to twelve digits
  so that the third point of `4:250:2000` is `1000.0`; the response is
  evaluated at the printed frequency. Phase in radians, principal value, not
  unwrapped: unwrapping on a log grid is a guess.
- **Truncation**: the share of the energy in the last tenth of the window is
  always printed; above `1e-6` a note gives the order of the error (its square
  root) and says to raise `-n`.
- The rows keep `outN` names, as a sweep's do, and `--eval` adds its legend.

Exit criterion. A one-pole and the TPT ladder match their closed forms to
`1e-12` of the passband's unit gain, in level and phase, at two sample rates,
at resonance 0 (`H1^4`: 12.04 dB down and half a turn late at the cutoff) and
at resonance 2 (`H1^4 / (1 + k H1^4)`); `x - x^3/3` and `ef.cubicnl` are
refused. Against the route this replaces, four sine renders with `--reduce
rms` on the resonant ladder, the two agree to `1e-13` dB. `--eval
'fi.peak_eq(6, 1000, 200)' --freqresp 256 stdfaust.lib` takes 30 ms and no
file.

Gates: the crate's 354 tests; `--protocol impulse-test` byte-identical to
`impulse-cranelift` on the 133 corpus programs.

Checks: `tests/freqresp_probe.rs` (16 tests), 8 unit tests of the module, 1 of
the weighted-impulse excitation. Eighteen mutations rejected: the checks
skipped (the plan's); each of the three left out; homogeneity with a positive
factor; the two impulses of superposition far apart; the phase conjugated; the
frequencies those of 44 100 Hz whatever `--sr`; `10 log10`; the tail taken at
the start of the window; `--in impulse:CH` exciting every input; `--settle`
moving the window and not the impulse; a linear grid; `--linearity-tolerance`
ignored; silence never rendered; any output with no input blamed; ringing
never noted; weighted impulses ignoring their channel. The positive-factor one
was first applied to the excitation alone, which refuses every program and so
survived a test that expects a refusal; applied to the excitation and the
expectation together, it is rejected by the rectifier. One test of mine was
wrong on the way (four stages at their cutoff are a quarter, -12.04 dB, not
-6.02).

