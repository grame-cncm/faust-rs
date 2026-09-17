---
title: "faustprobe: user guide"
date: 2026-08-16
page-size: A4
margins: 20 22
page-numbers: true
font-body: Roboto
font-heading: Roboto Condensed
font-mono: Roboto Mono
---

**Date:** 2026-08-16

**Audience:** anyone who needs to render a Faust DSP offline and get numbers
out of it — checking that a filter is stable, that an oscillator does not
alias, that a compressor's attack lands where it should, or that a change to a
library function did not move its output.

**Companion:** the design note
[`faustprobe-generic-test-tool-design-2026-08-14-en.md`](../porting/faustprobe-generic-test-tool-design-2026-08-14-en.md),
which explains why the tool exists and what it deliberately does not do.

---

## 1. What it is

`faustprobe` compiles a `.dsp` through the Cranelift JIT, renders it offline
with a chosen excitation, and prints either the samples or a summary. It sets
controls, schedules changes at exact frames, sweeps parameters, and reduces a
render to a single number per channel.

It is a *measuring* tool. It hands over samples and scalars; it does not plot,
does not compare against a reference, and stops short of anything that needs a
spectrum rather than a number. Where that boundary falls is §9.

```
faustprobe [OPTIONS] <FILE>
```

Exit status is `0` on success and `1` on any error — an unresolvable control, a
value outside its control's range, a malformed argument, a program that does
not compile, a render that produced non-finite samples or went above
`--fail-above`. That makes it usable directly in a shell gate.

## 2. First contact

```bash
faustprobe -I /path/to/faustlibraries filter.dsp
```

With no other flag this renders 15 000 frames at 44 100 Hz, feeds an impulse to
every input, and prints CSV on stdout with a `#`-prefixed summary on stderr:

```
frame,out0
0,1.0
1,0.78
2,0.6083999
…
# frames=15000 sr=44100 window=0..15000 (15000 frames)
# out0: peak=1.0 rms=0.013047670922390269 dc=0.0003030302538287278 finite=yes peak_at=0
```

(`filter.dsp` is `process = _ : + ~ *(0.78);`, in single precision.) `peak_at` is
the frame of the first sample that reached the peak, `none` for a silent
render: whether a maximum is the onset, a control event or a level still rising
at the end of the render is the first thing to know about it.

The split matters when piping: `> samples.csv` keeps the data and lets the
summary reach the terminal.

## 3. Compiling and rendering

| Flag | Meaning |
|---|---|
| `-I, --import-dir DIR` | Faust library search path, repeatable |
| `--double` | double-precision samples |
| `--opt-level N` | Cranelift optimisation level (default 0) |
| `--sr HZ` | sample rate (default 44100) |
| `--block N` | frames per `compute` call (default 64) |
| `--bra-tape N` | samples one `rad` reverse tape holds, the largest `--block` over which the gradients of a `rad` through delays and recursions are exact (default 8192, a power of two; the compiler's `-bra-tape`) |
| `-n, --render N` | frames to render (default 15000) |
| `--fail-above LEVEL` | fail when a sample of the window exceeds `LEVEL` in magnitude, and say where first (see "When a render fails") |

### When the program does not compile

Nothing is rendered, the exit status is `1`, and the error on stderr is the
compiler's complete diagnostic, the one `faust-rs` prints: a summary line, then
the location, the source line with its markers, the notes and the suggested fix.

```
faustprobe: parse failed for reverb.dsp: errors=1, recoveries=0, diagnostics=1
reverb.dsp:2:21: error [FRS-PARSE-0001] Parsing error at line 2 column 21. Repair sequences found:
   1: Insert RPAR
  2 | process = _ : *(0.5 ;
    |                     ^ unexpected token
    |                ^ `(` opened here
  = fix (machine-applicable): insert `)`
```

So the probe answers "does it compile?" as well as `faust-rs` does, and for the
path it measures: the Cranelift JIT in the width asked, which is not the path
`faust-rs -lang cpp` takes. `faustprobe -n 1 file.dsp` is the shortest such
check. (The probe compiles through the C API, whose `error_msg` buffer is 4096
bytes by contract and carries the summary line only; the rest comes from
`getCCompleteCraneliftDSPFactoryError`, which any host of that API can call.)

### Evaluating an expression: `--eval`

`--eval EXPR` probes an expression evaluated **in the scope of the file**
instead of the file's `process`. The file may be a `.lib`, which has no
`process` and could not otherwise be given to the tool; the expression sees its
definitions unprefixed, as inside the library, and its imports:

```
$ faustprobe --double -n 1 --in zero \
      --eval 'absorb_pole_exact(1709, 2.0, 0.5)' --eval 'absorb_pole(1709, 2.0, 0.5)' jot.lib
frame,"absorb_pole_exact(1709, 2.0, 0.5)","absorb_pole(1709, 2.0, 0.5)"
0,0.1981164814632339,0.5019283066233194
```

A question about a sub-expression costs a command, not a file with an `import`
and a `process`. The flag repeats, the expressions' outputs standing side by
side in the order given; an expression with several outputs gets `EXPR[0]`,
`EXPR[1]`, … (the columns are attributed by the number of outputs of each
expression, which a second, tiny program computes with `outputs(EXPR)`). A
header field that holds a comma is quoted, as CSV has it. With the statistics
comes a legend, `# eval out1 = EXPR`, which is what names the outputs under
`--quiet`, in a sweep (whose columns keep their `REDUCTION_outN` names) and in
JSON (an `eval` array).

Everything else applies to the program the expressions make: an expression
with inputs is a processor and is fed by `--in`
(`--eval 'fi.lowpass(2, 1000)' filters.lib` is an impulse response); its
controls are listed, set and swept, under the paths they have in the file, and
only those the expressions use exist; `--out`, `--fail-above` and `--train`
work as usual. `--eval '0' file.lib` is the compile check of a library.

What an expression sees is the file's **top-level** definitions: one local to
a `with` block is not in scope, and the error says so. The file's own `process`
is not evaluated at all, so a program that does not define one, or whose
`process` is broken, can still be asked about its parts.

When the compilation fails, a location in the file is the file's own line (the
wrapper `--eval` puts around the file shares its first line, whose columns
alone are shifted), and a location in an expression is `<eval k>`, with the
expression as its source line:

```
$ faustprobe --eval scale --eval 'third(1) + quarter' small.lib
faustprobe: evaluation failed for small.lib: undefined symbol `quarter`
<eval 1>:1:12: error [FRS-EVAL-0002] undefined symbol `quarter`
  8 | third(1) + quarter
    |            ^^^^^^^ failing use
  1 | __faustprobe_env = environment{ // a small library
    | ^^^^^^^^^^^^^^^^ enclosing definition
  …
  = note: <eval 1> is `--eval 'third(1) + quarter'`, line 8 of the source as wrapped
  = note: `__faustprobe_env = environment{...` and `process = ...` are the wrapper `--eval` puts around the file
```

A literal argument is folded at compile time, a control is computed at run
time, and the two can differ in the last bit:
`absorb_pole(967, 2.0, 0.5)` reads `0.28400507460781155` and the same function
of two sliders at 2.0 and 0.5 reads `0.2840050746078115`. `--eval` is how to
tell which one a program's output is. It does not combine with `--nvoices` or
the impulse-test protocol.

`--double` is worth reaching for whenever the measurement is near the noise
floor, or when the DSP evaluates trigonometric functions of a large argument —
single precision loses accuracy there and the loss can be mistaken for a defect
in the DSP.

`--block` changes how the render is chopped, not what it computes: a correct DSP
gives the same samples at any block size. A result that moves with `--block` is
itself a finding.

## 4. Excitation

`--in MODE` chooses what enters the DSP:

| Mode | Signal |
|---|---|
| `zero` | silence — the right choice for a generator, which needs no input |
| `impulse` | 1 on the first frame of every input, then 0 (the default) |
| `impulse:CH` | the same, on channel `CH` only |
| `dc` | constant 1 |
| `white[:SEED]` | white noise; the seed makes it reproducible |
| `sine:HZ` | a sine at `HZ` |
| `file:PATH[:CH]` | the channels of an audio file, `.wav` (PCM or float), `.f64` or `.f32` (raw little-endian, mono); input `i` reads channel `i`, a mono file feeds every input, `:CH` picks one channel for all; silence past the end. The samples are read at `--sr`: give the file's rate there (a WAV at another rate gets a warning; a raw file carries none) |

```bash
faustprobe --in zero -n 4 gen.dsp          # a generator drives itself
faustprobe --in "white:7" reverb.dsp       # reproducible noise
faustprobe --in "sine:1000" clipper.dsp    # drive a nonlinearity
```

`--skip N` drops the first `N` frames from both the dump and the statistics,
which is how a start-up transient is excluded. `--every N` prints one frame in
`N`, for eyeballing a long render.

## 5. Controls

`--list-params` shows what the DSP exposes, with the kind of every entry
(`slider`, `nentry`, `checkbox`, `button`, `bargraph`), and exits:

```
$ faustprobe --list-params synth.dsp
path                                         kind            init        min        max       step
/osc/freq                                    slider         440.0       50.0     2000.0       0.01
/osc/gain                                    slider           0.5        0.0        1.0      0.001
/osc/level                                   bargraph         0.0        0.0        1.0        0.0
```

A bargraph is listed with the controls because it shares their address
space, but it is an output of the program: see "Bargraphs" below.

`--set PATH=VALUE` writes a control before rendering, repeatable. `PATH` may be
a full address or a trailing fragment of one, so `--set freq=100` finds
`/osc/freq`. An ambiguous fragment is reported rather than resolved arbitrarily:

```
$ faustprobe --set gain=1 stereo.dsp
faustprobe: `gain` is ambiguous, matches: /amb/left/gain, /amb/right/gain
```

Everything a render will write is checked before any render: an unknown
path, an ambiguous fragment, a bargraph or **a value outside the control's
range** in `--set`, `--sweep` or `--at` is an error, not a render:

```
$ faustprobe --sweep gain=0.5,1,7 --reduce peak synth.dsp
faustprobe: `gain`=7 is outside the range [0, 1] of /osc/gain (--clamp accepts it, clamped to the range)
```

A Faust host never writes outside a widget's range and a DSP is not compiled to
expect it, so the render would clamp the value; and a render at 1 in a row
labelled 7 looks like a measurement of 7. `--clamp` accepts such a value, and
says what it did: `# clamped /osc/gain: 7 -> 1` with the statistics (on stderr
before a sweep's rows), a `clamped` array in the JSON runs concerned, and sweep
rows that carry the value used, `1`, not the one asked for. With `--train` the
same holds for `--set`: a starting point outside the range is an error.
`--nvoices` keeps the polyphonic wrapper's own rule (a voice's controls are
written unclamped, as `poly-dsp.h` does) and refuses `--clamp`.

`--at FRAME PATH=VALUE` writes a control at an exact frame. The render splits
its block so the change lands on the requested frame rather than at the next
block boundary — which is what makes an attack measurable:

```bash
faustprobe --at 0 gate=1 --at 1 gate=0 --in zero -n 5000 pluck.dsp
```

That pair is the idiom for a one-sample trigger on a `button`.

### Bargraphs

A `hbargraph` or `vbargraph` is how a Faust program shows a value: a level, a
detector's state, a learned coefficient, the parameters a preset selected.
The program writes it, once per sample; a host reads it. `faustprobe` reads
every bargraph after a render and reports it with the statistics,

```
$ faustprobe --double --quiet --set preset=4 jot_presets_bargraph.dsp
# frames=15000 sr=44100 window=0..15000 (15000 frames)
# out0: peak=1.2899768930149773 rms=0.04545460382493471 dc=0.0005037494887966796 finite=yes peak_at=1510
# out1: peak=1.2899768930149773 rms=0.04262056900393426 dc=0.00012935283157617836 finite=yes peak_at=1510
# bargraph /jot_presets_bargraph/T60_at_dc=1.9215
# bargraph /jot_presets_bargraph/T60_at_half_the_sample_rate=0.527
# bargraph /jot_presets_bargraph/gain=29.954550372874973
# bargraph /jot_presets_bargraph/long_lines=1.0
```

and, in `--format json`, as a `bargraphs` object in every run, keyed by path
(the key is absent when the program has none; the schema version is
unchanged, the key being an addition). `--bargraphs` puts them in the rows as
well: one column per bargraph, named by its path, after the outputs in the
per-frame CSV dump and after the reductions in a sweep's rows, where it is the
value at the end of each point's render:

```
$ faustprobe --double -n 2000 --sweep preset=0,4,7 --reduce rms --bargraphs jot_presets_bargraph.dsp
preset,rms_out0,rms_out1,/jot_presets_bargraph/T60_at_dc,...
0,0.048635476498202976,0.04863887969523716,0.29,...
4,0.06132501143284588,0.06132501143284588,1.9215,...
7,0.024210882299051106,0.024210882299051106,6.9848,...
```

A bargraph's zone holds the value of the last sample of the last `compute`
call, so in a per-frame dump a row carries the value at the end of the block
its frame belongs to: the time resolution is `--block` (and the blocks that
`--at` splits). Lower `--block` to follow a meter more finely. A bargraph
cannot be written: `--set`, `--sweep` and `--at` refuse one,

```
$ faustprobe --set level=1 synth.dsp
faustprobe: `/osc/level` is a bargraph, an output of the program: it cannot be set
```

since the program would overwrite the value at the next block and a sweep
over it would print identical rows that look like a measurement. `--bargraphs`
does not combine with `--format ir`, `--train`, the impulse-test protocol or
`--nvoices`.

## 6. Output formats

`--format` selects what the frames look like.

**`csv`** (default) is `frame,out0,out1,…`, directly pipeable.

**The text of a number** is, everywhere but in `.ir`, the shortest decimal
string that parses back to the same float **at the width the program was
compiled in**, plain or scientific according to the magnitude: `0.78`,
`0.6083999`, `3.3333333333333334e-8`, `NaN`, `inf`. A script that reads it
with `float()` gets the very sample, so a comparison with a reference can use
the tolerance the programs deserve rather than one set by the printing. Samples,
peaks, bargraphs and a control's bounds are at the program's width (an `f32` is
printed as the `f32` it is, `0.001`, not as its double,
`0.0010000000474974513`); means, RMS values, reductions and trained controls
are computed in `f64` and printed as such. `--precision N` prints `N` fixed
decimals instead, and `--precision 9` is the text this tool printed before it
had the flag, for anything pinned to it. Fixed decimals lose small values,
which is why they are no longer the default: `3.3e-8` prints `0.000000033`,
two significant digits, in either width.

**`--out FILE`** writes the rendered window (after `--skip`) to a file and
leaves only the statistics on stdout: the way to hand a long render to a
script, binary and exact instead of one decimal string per sample.

| Extension | Content |
|---|---|
| `.npy` | NumPy array, shape `(frames, outputs)`, `<f8` or `<f4` by width: `numpy.load` returns what parsing the CSV used to build |
| `.wav` | IEEE float, 64 or 32 bits, every output, with the sample rate; `--in file:` reads it back, so a render becomes the excitation or the reference of another |
| `.f64`, `.f32` | raw little-endian samples of a program with **one** output, the layout `--in file:` reads; `.f32` is refused for a double-precision program |

Every frame of the window is written: `--out` refuses `--every`, and also
`--sweep`, `--train`, `--format ir` and `--nvoices`.

**`ir`** reproduces the reference impulse-test text, header and zero-clamp
included, for byte comparison against the existing corpus:

```
number_of_inputs  :   1
number_of_outputs :   1
number_of_frames  :      3
     0 :  1.000000
```

**`json`** emits one versioned object. It is the format that carries the full
structure of a sweep.

`--quiet` suppresses the per-frame dump and prints only the statistics. Under
`--quiet` (or `--out`) those statistics *are* the output, so they go to stdout
and can be redirected; without it they annotate a dump that already owns stdout
and go to stderr.

### When a render fails

A render with a non-finite sample is an error (except in `--format ir`, where
the artifact is what is judged), and the error says where it starts:

```
$ faustprobe --in dc -n 4000 --quiet --set drive=2 --at 1000 g=1.5 --at 3000 g=0.2 loop.dsp
faustprobe: render produced non-finite samples
  first: frame 1213, out0 (+inf); 2787 of 4000 frames affected
  controls written by then: /loop/drive=2 /loop/g=1.5
  last scheduled write before it: frame 1000, /loop/g=1.5
```

The first frame and its output, how many frames are affected (once and
recovered, or for good), the controls the command line had written by that
frame with their values then (the others are at their initial values), and the
last `--at` before it; a write scheduled after the failure is not listed.

A feedback loop that leaves its stable region runs away for hundreds of frames
before it overflows. `--fail-above LEVEL` fails the render at the first sample
of the window whose magnitude exceeds `LEVEL`, which is where to look:

```
$ faustprobe … --fail-above 1000 loop.dsp
faustprobe: a sample exceeds --fail-above 1000
  first: frame 1011, out0 = 1033.9707
  the render turns non-finite at frame 1213, out0 (+inf)
  controls written by then: /loop/drive=2 /loop/g=1.5
  last scheduled write before it: frame 1000, /loop/g=1.5
```

It also turns "stays bounded under these control changes" into an exit status:
schedule the jumps with `--at`, set a level no sane signal reaches. The level is
checked in the window only (a transient before `--skip` is not measured),
non-finite samples everywhere.

### Silence

When every output is **exactly** zero over the window, the statistics come with
the facts the tool has that explain it, as `# note:` lines (a `notes` array in
JSON; once, on stderr, for a sweep that is silent at every point):

```
$ faustprobe -n 4096 --quiet synth.dsp
# out0: peak=0.0 rms=0.0 dc=0.0 finite=yes peak_at=none
# note: every output is exactly zero over the window
# note: buttons and checkboxes at 0: /synth/gate
```

The other notes are `input is `zero` and the program has N input(s)` and, with
`--nvoices`, `no --note or --chord is scheduled: every voice stays free`. The
exit status stays 0, silence being sometimes the right answer, and a quiet
signal is not a silent one: the comparison is with zero, not with a threshold.

## 7. Sweeps and reductions

`--sweep PATH=V1,V2,…` renders once per value. Repeating the flag takes the
cartesian product, with the **last axis varying fastest**:

```
$ faustprobe --precision 9 --sweep freq=100,200 --sweep gain=0.1,0.9 --reduce peak dsp.dsp
freq,gain,peak_out0
100,0.1,0.099999368
100,0.9,0.899994314
200,0.1,0.099999368
200,0.9,0.899994314
```

Every point renders from a cleared instance, so one configuration cannot
contaminate the next.

`--sweep` combines with `--at`, which is what measuring a *triggered* instrument
against a swept parameter requires — attack level against pitch, for example.
The one rejected combination is a schedule that writes a control the sweep is
also driving, since the scheduled write would silently override the swept value
and the reported axis would not be what the render used.

`--reduce R` collapses each render to one number per channel:

| Reduction | Meaning |
|---|---|
| `rms` | root mean square over the window |
| `peak` | largest absolute value |
| `energy` | sum of squares |
| `dc` | mean — non-zero flags an offset |
| `f0` | frequency of the strongest non-DC bin |
| `sfdr` | spurious-free dynamic range (§8) |
| `thd` | total harmonic distortion (§8) |

With `--format csv` a sweep prints one row per point, as above. With
`--format json` it prints the full structure, including the window each point
used. `--format ir` cannot hold a sweep and is rejected.

## 8. Measuring aliasing and distortion

`sfdr` and `thd` answer opposite questions about the same spectrum.

**`sfdr`** — spurious-free dynamic range — is the distance in dB from the
fundamental down to the loudest component *off* its harmonic grid. Larger is
cleaner. This is the measurement for a band-limited oscillator or an
antialiased waveshaper, where the harmonics are wanted and everything else is
not:

```
$ faustprobe --precision 9 --f0 187.5 --sweep k=1,3,6,14 --reduce sfdr --skip 2048 -n 10240 gen.dsp
k,sfdr_out0
1,303.736996229
3,304.525727177
6,304.945901462
14,306.857783832
```

**`thd`** is the companion and the opposite question: the energy in harmonics 2,
3, … relative to the fundamental. Here the harmonics are what is measured rather
than what is excluded — the right choice for characterising a saturator.

Both need a fundamental. `--f0 HZ` pins it; without it the strongest bin is
used, which is wrong for any signal whose loudest partial is not the fundamental
— a bright pluck, a filtered saw.

Two properties decide whether the number means anything.

**The window sets the floor.** Both use a Blackman-Harris window, whose
sidelobes are 92 dB down. An arbitrary tone therefore reads about **93 dB SFDR
however clean the DSP is**, and a result near that number measures the transform
rather than the signal. Choosing a frame count that puts `f0` on a bin centre
removes the leakage and takes the floor to numerical precision — that is why the
example above reads 304 dB.

**The window must be stationary.** Measuring while a spectrum decays smears
every partial, and the smearing appears as off-grid energy: a decaying pluck can
read 20 dB while being perfectly alias-free. Use `--skip` and `-n` to select a
steady stretch.

## 9. Where the tool stops

A `--reduce` returns one scalar per channel. That covers every property that can
gate a build: level, offset, dominant frequency, aliasing, distortion, and any
of them across a parameter sweep.

What it does not cover is anything needing a *vector*. Comparing a hundred
partials against a predicted curve, or tracking each of their decay slopes over
time, asks for a spectrum, and no further reduction can supply it. Those belong
in an analysis script reading the CSV — which is the intended division of
labour, not a missing feature.

## 10. Polyphony

`--nvoices N` compiles `N` instances from one JIT and drives them through the
polyphonic wrapper ported from `poly-dsp.h`: allocation, stealing, mixing, and
reclamation of a releasing voice once it falls below `--voice-stop-level`
(default `0.00003162`, i.e. −90 dB, the value from `poly-dsp.h`).

```bash
faustprobe --nvoices 4 --note "60@0" --note "64@2000" -n 8000 --quiet synth.dsp
```

`--note PITCH[:VEL]@ON[..OFF]` plays one note; velocity defaults to 100, and
omitting `..OFF` holds it to the end of the render, which is how an attack is
measured without a release in the way. `--chord P1,P2,…[:VEL]@ON[..OFF]` plays
several pitches at once.

`--effect FILE` runs a separate effect DSP on the mixed output. A single file
declaring both `process` and `effect` has its effect extracted automatically,
the way `FaustPolyDspGenerator` does, so the flag is only needed to override
that guess or to pair files.

## 11. The impulse-test protocol

`--protocol impulse-test` pins every rendering condition to the reference
values — 44 100 Hz, block 64, impulse on every input, buttons held for the first
block, `.ir` output — and **rejects any flag that would perturb them**:

```
$ faustprobe --protocol impulse-test --sr 48000 dsp.dsp
faustprobe: --protocol impulse-test fixes the rendering conditions; remove --sr
```

Refusing rather than silently overriding is the point: a regression run that
was quietly mis-configured produces a `.ir` that looks valid and compares wrong.

One deliberate asymmetry in this mode: a non-finite sample is an error
everywhere else, but not here. The reference corpus contains DSPs whose expected
output contains NaN, and the artifact is what the comparison judges — the exit
code says whether the render was produced, not whether the DSP diverged.

## 12. Recipes

**Is this filter stable?**

```bash
faustprobe --in impulse -n 200000 --quiet filter.dsp
```

A `peak` that grows with `-n`, or `finite=no`, is the answer.

**Does this oscillator alias?**

```bash
faustprobe --in zero --f0 3000 --reduce sfdr --skip 4096 -n 12288 osc.dsp
```

Read §8 first: pin `--f0`, and prefer a frame count that puts it on a bin
centre.

**Where does this compressor's gain settle?**

```bash
faustprobe --in "sine:1000" --at 0 "threshold=-20" --skip 20000 --reduce rms comp.dsp
```

**Did this library change move anything?**

```bash
faustprobe --protocol impulse-test dsp.dsp > new.ir && diff old.ir new.ir
```

**How does a parameter affect the output?**

```bash
faustprobe --sweep cutoff=100,200,400,800,1600 --reduce rms --in "white:1" filt.dsp
```

## 13. Host loops: `--train` and `--fd-check`

Some programs learn nothing themselves: they output a loss and, from `rad`,
the per-sample contributions of its gradient with respect to their sliders,
and leave the optimisation to a host (`tests/corpus/ddsp_rad_host_block_resonator.dsp`,
`ddsp_rad_gru_amp_host.dsp`, see `libraries/ddsp-examples-en.md`). The block
reverse sweep makes the sum of a gradient lane over a `compute` block the
gradient of the block's loss. `--train` is that host:

```bash
faustprobe --double -I libraries -I <faustlibraries> --in white:1 --block 256 \
    --train a1,a2 --fd-check --lr 0.01 --blocks 600 --every 100 \
    tests/corpus/ddsp_rad_host_block_resonator.dsp
```

`--train` names the controls, exact paths or unique suffixes, in the order
of their gradient lanes; `--loss-lane` (default 0) and `--grad-lane`
(default 1, the lanes of the controls follow it) say where the lanes are;
`--optimizer adam|sgd`, `--lr` and `--blocks` set the loop, `--block` the
block size, `--in` the excitation (`white:SEED` for a reproducible one,
`file:PATH` for a recording). Per block, the controls are written, the
block computed on the same instance (the state carries across blocks:
truncated backpropagation through time for a recurrent model), the loss
and gradient lanes averaged, the controls stepped and kept in their range.
With `--reset-per-block` every block starts instead from a cleared state
and from frame 0 of the excitation: one pass over the same response per
block, the offline calibration of a program whose target is a measured
response given by `--in file:` and whose block is the whole response
(`--block` its length, `--bra-tape` the next power of two, `--sr` the
rate of the recording). One CSV row per block, thinned by
`--every`, then the trained values and the first and last loss:

```text
block,loss,a1,a2
100,2.388789543804934e-4,-1.1946213530521381,0.7152390374663387
200,2.7272607473475966e-10,-1.2000005084309235,0.7200039032015184
...
600,3.462933533296437e-28,-1.1999999999999915,0.7199999999999901
# trained /ddsp_rad_host_block_resonator/a1=-1.1999999999999915
# trained /ddsp_rad_host_block_resonator/a2=0.7199999999999901
# loss: block 1 4.615726e-1, block 600 3.462934e-28
```

`--fd-check` runs first (or alone, with `--blocks 0`): each gradient lane,
summed over one block from a fresh instance at the controls' initial
values, against the central finite difference of the summed loss lane with
step `--fd-step` (default 1e-3); the command fails when a relative error
`|rad - fd| / max(|fd|, 1)` exceeds `--fd-tolerance` (default 0.02). It is
the first thing to run when a gradient looks wrong:

```text
# fd-check /ddsp_rad_host_block_resonator/a1: rad 476.523699 fd 476.521802 relative error 3.98e-6
# fd-check /ddsp_rad_host_block_resonator/a2: rad 335.872145 fd 335.873239 relative error 3.26e-6
# fd-check: block 256 frames, step 0.001, worst relative error 3.98e-6 (tolerance 0.02)
```

The GRU of the second example trains its 27 sliders the same way,
`--train wz1,wz2,...,bo --lr 0.005 --blocks 2000`, in a fraction of a
second; `--sweep`, `--reduce`, `--at` and the impulse-test protocol do not
combine with it.

`--set` does, with two meanings. On a trained control it is the descent's
starting point, in place of the slider's initial value: `--set a1=-0.4
--train a1,a2` leaves from `-0.4` (a value outside the control's range is an
error, or under `--clamp` a reported clamp, as for a render), and `--fd-check`
checks the gradients there. On any other control it
is a fixed value, rewritten on every fresh instance and after every
`--reset-per-block` reset, since a reset restores the widgets' defaults: the
way to fit a program whose other controls select a variant (`--set exact=1`)
without editing it. A starting point found by a `--sweep` of the loss over a
grid, then a descent from it, is the grid-then-gradient of
`libraries/optimizers-overview-en.md` done by the host.
