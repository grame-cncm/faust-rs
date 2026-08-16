---
title: "faust-probe: user guide"
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
[`faust-probe-generic-test-tool-design-2026-08-14-en.md`](../porting/faust-probe-generic-test-tool-design-2026-08-14-en.md),
which explains why the tool exists and what it deliberately does not do.

---

## 1. What it is

`faust-probe` compiles a `.dsp` through the Cranelift JIT, renders it offline
with a chosen excitation, and prints either the samples or a summary. It sets
controls, schedules changes at exact frames, sweeps parameters, and reduces a
render to a single number per channel.

It is a *measuring* tool. It hands over samples and scalars; it does not plot,
does not compare against a reference, and stops short of anything that needs a
spectrum rather than a number. Where that boundary falls is §9.

```
faust-probe [OPTIONS] <FILE>
```

Exit status is `0` on success and `1` on any error — an unresolvable control, a
malformed argument, a render that produced non-finite samples. That makes it
usable directly in a shell gate.

## 2. First contact

```bash
faust-probe -I /path/to/faustlibraries filter.dsp
```

With no other flag this renders 15 000 frames at 44 100 Hz, feeds an impulse to
every input, and prints CSV on stdout with a `#`-prefixed summary on stderr:

```
frame,out0
0,1.000000000
1,0.780000000
…
# frames=15000 sr=44100 window=0..15000 (15000 frames)
# out0: peak=1.000000000 rms=0.031622777 dc=0.000158114 finite=yes
```

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
| `-n, --render N` | frames to render (default 15000) |

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

```bash
faust-probe --in zero -n 4 gen.dsp          # a generator drives itself
faust-probe --in "white:7" reverb.dsp       # reproducible noise
faust-probe --in "sine:1000" clipper.dsp    # drive a nonlinearity
```

`--skip N` drops the first `N` frames from both the dump and the statistics,
which is how a start-up transient is excluded. `--every N` prints one frame in
`N`, for eyeballing a long render.

## 5. Controls

`--list-params` shows what the DSP exposes and exits:

```
$ faust-probe --list-params synth.dsp
path                                               init        min        max       step
/osc/freq                                           440         50       2000       0.01
/osc/gain                                           0.5          0          1      0.001
```

`--set PATH=VALUE` writes a control before rendering, repeatable. `PATH` may be
a full address or a trailing fragment of one, so `--set freq=100` finds
`/osc/freq`. An ambiguous fragment is reported rather than resolved arbitrarily:

```
$ faust-probe --set gain=1 stereo.dsp
faust-probe: `gain` is ambiguous, matches: /amb/left/gain, /amb/right/gain
```

`--at FRAME PATH=VALUE` writes a control at an exact frame. The render splits
its block so the change lands on the requested frame rather than at the next
block boundary — which is what makes an attack measurable:

```bash
faust-probe --at 0 gate=1 --at 1 gate=0 --in zero -n 5000 pluck.dsp
```

That pair is the idiom for a one-sample trigger on a `button`.

## 6. Output formats

`--format` selects what the frames look like.

**`csv`** (default) is `frame,out0,out1,…` at full precision, directly pipeable.

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
`--quiet` those statistics *are* the output, so they go to stdout and can be
redirected; without it they annotate a dump that already owns stdout and go to
stderr.

## 7. Sweeps and reductions

`--sweep PATH=V1,V2,…` renders once per value. Repeating the flag takes the
cartesian product, with the **last axis varying fastest**:

```
$ faust-probe --sweep freq=100,200 --sweep gain=0.1,0.9 --reduce peak dsp.dsp
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
$ faust-probe --f0 187.5 --sweep k=1,3,6,14 --reduce sfdr --skip 2048 -n 10240 gen.dsp
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
faust-probe --nvoices 4 --note "60@0" --note "64@2000" -n 8000 --quiet synth.dsp
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
$ faust-probe --protocol impulse-test --sr 48000 dsp.dsp
faust-probe: --protocol impulse-test fixes the rendering conditions; remove --sr
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
faust-probe --in impulse -n 200000 --quiet filter.dsp
```

A `peak` that grows with `-n`, or `finite=no`, is the answer.

**Does this oscillator alias?**

```bash
faust-probe --in zero --f0 3000 --reduce sfdr --skip 4096 -n 12288 osc.dsp
```

Read §8 first: pin `--f0`, and prefer a frame count that puts it on a bin
centre.

**Where does this compressor's gain settle?**

```bash
faust-probe --in "sine:1000" --at 0 "threshold=-20" --skip 20000 --reduce rms comp.dsp
```

**Did this library change move anything?**

```bash
faust-probe --protocol impulse-test dsp.dsp > new.ir && diff old.ir new.ir
```

**How does a parameter affect the output?**

```bash
faust-probe --sweep cutoff=100,200,400,800,1600 --reduce rms --in "white:1" filt.dsp
```
