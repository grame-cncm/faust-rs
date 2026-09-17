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
and a plan in five phases. **Status: F1 is implemented (2026-09-17, §7); F2 to F5
are not.**

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

