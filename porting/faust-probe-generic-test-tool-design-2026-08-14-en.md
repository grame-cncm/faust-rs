# `faust-probe` — Generic DSP Probing Tool (Cranelift, Polyphonic) — Design

Date: 2026-08-14

C++ reference: `master-dev-ocpp-od-fir-2-FIR19` at `8eebea429`
(`/Users/letz/Developpements/RUST/faust`)

Relevant existing binaries:
[`crates/impulse-runner/src/main.rs`](../crates/impulse-runner/src/main.rs) (interpreter),
[`crates/cranelift-ffi/src/bin/impulse_cranelift.rs`](../crates/cranelift-ffi/src/bin/impulse_cranelift.rs) (Cranelift).

## 1. Purpose

Both existing runners answer one question: **did the behaviour change?** They
render a fixed protocol — SR 44100, block 64, impulse on frame 0, buttons held
for the first block only, sliders left at their defaults — and emit `.ir` text
for `filesCompare -part` against the C++ oracle. The protocol is fixed *by
construction*: regression testing requires that nothing varies.

They cannot answer the other question: **is the behaviour correct?** That
requires varying exactly the thing under test — setting a cutoff to 1 kHz with
resonance at 0 to compare against an analytic transfer function, holding a gate
for 44 000 samples to measure a release time, putting an impulse on the left
channel only to observe a ping-pong delay.

This gap was hit concretely while porting a C++ synthesizer engine to Faust
(`agentic-synth`, branch `faust`). Verification there required a bespoke
architecture file — `faust -a harness.cpp` plus a `c++ -O2` compile per DSP
variant, 3-5 s each — because no faust-rs tool could set a parameter. Of the
eight validation properties that port needed, four were unreachable with
`impulse-runner`:

| Property | Reachable today |
|---|---|
| Self-oscillation at resonance 1 | yes — impulse then silence is its protocol |
| SVF mode responses | yes, with one purpose-built `.dsp` per mode |
| PolyBLEP aliasing floor | yes, with one `.dsp` per frequency |
| Bit-exact bypass | yes |
| Ladder magnitude vs analytic theory | yes — and better than a sine sweep (§7.1) |
| **Envelope release timing** | **no** — gate lasts 64 samples, 44 000 needed |
| **Delay ping-pong** | **no** — impulse goes to *every* input channel |
| **M/S width collapse** | **no** — same reason |

`faust-probe` is the tool that answers both questions: the controllability of a
bespoke harness, the zero-build execution of the existing runners, and — new to
both — parameter sweeps inside a single process.

## 2. Why Cranelift, and why it changes the design

The build step is what shapes a test tool. A bespoke C++ harness costs a
compile per configuration, which pushes the design toward *one DSP with sliders
plus many command lines*. Remove the compile and the design space opens: one
process can JIT once and render hundreds of configurations.

That only pays if rendering itself is cheap. Measured on this machine, same DSP
(a 4-pole ladder at 1 kHz), same output:

| Frames | Interpreter | Cranelift |
|---|---|---|
| 15 000 | 0.07 s | 0.06 s |
| 96 000 | 0.11 s | 0.04 s |
| 2 000 000 | 1.96 s | 0.41 s |

Subtracting the ~0.05 s JIT/front-end baseline, Cranelift renders **~5.3x
faster** (1.91 s against 0.36 s). At 15 000 frames the two are
indistinguishable because compilation dominates — which is exactly why the
existing runners show no reason to prefer one backend.

The sweep primitive is what monetises that ratio. A 30-point sweep at 96 000
frames costs one JIT plus 30 renders: ~1.2 s under Cranelift against ~3.3 s
under the interpreter, and against ~150 s under the compile-per-variant scheme
the bespoke harness forced.

**Consequence for the design:** sweeps and reductions are not conveniences
bolted on later. They are the reason the tool is worth building in Cranelift
rather than extending `impulse-runner`.

## 3. Method

`impulse_cranelift` already establishes the runtime path to reuse: the
Cranelift C-FFI surface (`createCCraneliftDSPFactoryFromFile`,
`initCCraneliftDSPInstance`, `buildUserInterfaceCCraneliftDSPInstance` with a
`UIGlue`, `computeCCraneliftDSPInstance`), plus a 256 MB worker stack because
the front end recurses deeply.

Two things must be added on top of it.

### 3.1 A path to zone map

`UIGlue` ([`crates/ffi-common/src/abi.rs:111`](../crates/ffi-common/src/abi.rs))
delivers `open_vertical_box` / `open_horizontal_box` / `open_tab_box` /
`close_box`, and the `add_button` / `add_check_button` /
`add_vertical_slider` / `add_horizontal_slider` / `add_num_entry` callbacks,
each carrying its `*mut FfiFaustFloat` zone pointer. Accumulating the box stack
while walking those callbacks reconstructs the full address of every control —
the algorithm of `architecture/faust/gui/PathBuilder.h` and `MapUI.h`.

The result is `HashMap<String, *mut FAUSTFLOAT>` plus the `init`/`min`/`max`
metadata, which is everything `--set`, `--sweep` and `--at` need.

`getCCraneliftDSPFactoryJSON` also carries the addresses and ranges, and is
easier to parse, but it does not carry zone pointers. Use the `UIGlue` walk for
zones and the JSON only if a `--list-params` mode wants richer metadata.

**Suffix resolution.** Accept a trailing fragment (`filter_cutoff_hz`) as well
as a full address (`/TIMBRE/filter_cutoff_hz`), erroring on ambiguity and
listing candidates. Full addresses are unwieldy and change when a group is
renamed; this was the single most-used affordance of the bespoke harness.

### 3.2 A polyphonic wrapper

The interpreter runtime has no poly/MIDI wrapper — `impulse-runner`'s header
comment says so, and it is the reason that runner is scalar-only. Cranelift is
in the same position today. Polyphony therefore has to be built in the tool,
following `architecture/faust/dsp/poly-dsp.h`.

That file is small enough to port faithfully, and the parts that matter are:

**Voice-control discovery** (`dsp_voice::extractPaths`, poly-dsp.h:233).
Suffix match over the full-path map:

| Suffix | Role | Conversion |
|---|---|---|
| `/gate` | note on/off | 1.0 / 0.0 |
| `/freq` | pitch | `440 * 2^((note-69)/12)` |
| `/key` | pitch | identity (raw MIDI number) |
| `/gain` | velocity | `vel / 127` |
| `/vel`, `/velocity` | velocity | identity |

The `freq` vs `key` and `gain` vs `vel` distinction selects the conversion
function; getting it wrong silently detunes every note by an octave-scale
factor, so it is worth a test of its own.

**Voice states** (poly-dsp.h:52): `kFreeVoice -1`, `kReleaseVoice -2`,
`kLegatoVoice -3`, `kNoVoice -4`, with `fDate` a monotonic allocation counter
and `fLevel` the last block's level.

**Allocation** (`getFreeVoice`, poly-dsp.h:630): first voice whose `fCurNote ==
kFreeVoice`; failing that, steal the oldest *releasing* voice; failing that,
the oldest *playing* voice. A stolen voice enters `kLegatoVoice`, where
`computeLegato` renders the outgoing and incoming notes and fades out the first
half-buffer.

**Retrigger** (`getPlayingVoice`, poly-dsp.h:601): a note-on for a pitch already
sounding reuses the oldest voice playing it, rather than allocating.

**Mixing and voice reclamation** (`compute`, poly-dsp.h:828): each non-free
voice renders into a mix buffer; `mixCheckVoice` accumulates and returns the
level; a voice in `kReleaseVoice` whose level falls below `VOICE_STOP_LEVEL`
(`0.00003162`, i.e. -90 dB, poly-dsp.h:57) returns to `kFreeVoice`. Then the
effect DSP, if present, runs once on the sum.

That threshold is the one number here with an audible consequence: too high and
long releases are truncated, too low and voices are never reclaimed under
sustained play. It should be exposed as `--voice-stop-level` so a test can
assert against it rather than around it.

**`process` / `effect` pairing.** The tool must accept either one DSP compiled
twice (extracting `effect` the way `FaustPolyDspGenerator` does, by wrapping
the source in `environment{}` and taking `dsp_code.effect`) or two separate
files. The former matters because that is how a single-file polyphonic
instrument is written.

**Scope limit to state up front:** this is a *test* wrapper, not an audio
engine. No MIDI file input, no soundfile playback beyond the impulse-test
memory reader, no real-time thread. Anything the C++ `poly-dsp.h` does for
live performance that does not affect offline determinism should be omitted
rather than half-ported.

## 4. Interface

```
faust-probe <file.dsp> [options]

Compilation
  -I, --import-dir <DIR>        library path (repeatable)
  --double | --single           sample format (default single)
  --effect <FILE>               separate effect DSP for the poly bus
  -n, --nvoices <N>             polyphonic voices; 0 = scalar (default 0)

Controls
  --set PATH=VALUE              set before rendering (repeatable)
  --at FRAME PATH=VALUE         set at a frame index (repeatable)
  --sweep PATH=V1,V2,...        render once per value (repeatable, cartesian)
  --list-params                 print addresses, ranges, defaults; exit

Notes (requires --nvoices > 0)
  --note PITCH:VEL@ON..OFF      key on at ON, key off at OFF (repeatable)
  --chord P1,P2,...:VEL@ON..OFF shorthand for several --note

Rendering
  --render <FRAMES>             frames to render (default 15000)
  --sr <HZ>                     sample rate (default 44100)
  --block <N>                   block size (default 64)
  --in <MODE>                   zero|impulse|impulse:CH|dc|white[:SEED]|sine:HZ

Output
  --skip <N>                    exclude the first N frames from stats and dump
  --every <N>                   decimate the dump
  --reduce <R>                  rms|peak|energy|dc|f0|thd|none (default none)
  --format <F>                  csv|ir|json (default csv)
  --protocol impulse-test       reproduce the reference protocol exactly
```

Three points are load-bearing:

- **`--in impulse:0`** — an impulse on one channel. The reference protocol puts
  an impulse on *every* input, which makes a ping-pong delay untestable because
  both channels start identical.
- **`--at FRAME PATH=VALUE`** — scheduled parameter changes. Needed for
  crossfade and smoother behaviour, where the question is what happens *during*
  a change.
- **`--protocol impulse-test`** — pins SR 44100, block 64, impulse on all
  inputs, buttons on for the first block, `.ir` format and the `|x| < 1e-6`
  zero-clamp of `controlTools.h::normalize`, rejecting any flag that would
  perturb it. With this, `faust-probe` **subsumes** both existing runners
  instead of competing with them, and stays usable against the existing corpus
  and `filesCompare -part`.

### 4.1 Sweep output

A sweep must emit one row per configuration, not concatenated renders:

```
$ faust-probe timbre.dsp -I lib --set osc0_volume=1 --set filter_resonance=0 \
    --sweep filter_cutoff_hz=250,500,1000,2000,4000 \
    --render 96000 --skip 48000 --reduce rms --format json
```

```json
{"schema_version": 1, "dsp": "timbre.dsp", "sr": 44100,
 "runs": [{"set": {"filter_cutoff_hz": 250},  "rms": [0.0341, 0.0339]},
          {"set": {"filter_cutoff_hz": 500},  "rms": [0.0724, 0.0721]}]}
```

JSON so a property check is a comparison against computed expectations rather
than shell text munging. Versioned for the same reason the diagnostics contract
is.

## 5. Phases

| Phase | Content | Exit criterion |
|---|---|---|
| **P0** | This document | reviewed |
| **P1** | Path→zone map over `UIGlue`; `--list-params`, `--set`, `--render`, `--in`, `--skip`, CSV | reproduces the `agentic-synth` harness measurements: ladder magnitude matches analytic theory to 6 significant figures |
| **P2** | `--protocol impulse-test`, `.ir` format | byte-identical to `impulse_cranelift` on the existing corpus |
| **P3** | `--sweep`, `--reduce`, JSON output | a 30-point sweep runs in one process; timings match §2 |
| **P4** | Poly wrapper: discovery, allocation, stealing, retrigger, mixing, effect | note timing and voice-steal behaviour verified against the C++ `poly-dsp.h` semantics |
| **P5** | `--at`, `--note`, `--chord` | envelope release timing measurable: a 0.5 s release lands within 5% |

P1 and P2 are independently useful; P4 is the largest and should not gate them.

## 6. Validation

The tool is a measuring instrument, so it needs its own calibration — an
instrument that silently misreports is worse than none.

1. **Against the existing runners.** `--protocol impulse-test` output must be
   byte-identical to `impulse_cranelift`, and to `impulse-runner` modulo the
   6-decimal print and zero-clamp. A cross-check already performed by hand
   during the `agentic-synth` port: `impulse-runner` and a `faust -a` native
   harness agreed to printed precision on the same ladder, so three independent
   implementations can be pinned to each other.
2. **Against analytic theory.** A one-pole and a 4-pole TPT ladder at resonance
   0 have closed-form magnitude responses. `--sweep` plus `--reduce rms` on a
   sine input must match to float precision. This is the test that catches a
   wrong `--skip` semantic — see §7.2.
3. **Against the C++ poly engine.** For a small polyphonic DSP, note-on/note-off
   sequences rendered by `faust-probe` and by a `poly-dsp.h`-based C++ harness
   must agree. Voice stealing order is the interesting case.
4. **Determinism.** Two runs of the same command produce identical bytes,
   including with `--in white` (hence `white[:SEED]`).

## 7. Lessons carried over from the `agentic-synth` port

Two failures of the bespoke harness that this tool should be built not to
repeat.

### 7.1 Prefer the impulse response where the system is linear

The ladder's magnitude response was measured by rendering a sine at each of
four frequencies — four runs for information that a single impulse response
plus an FFT yields in full. `--reduce` should therefore offer a spectral
reduction, so the linear case is one run rather than a sweep.

This holds only while the system is linear: at resonance 1 the `tanh` in the
feedback path dominates and there is no transfer function, so the sine sweep
remains necessary. The tool should not pretend otherwise.

### 7.2 Statistics must honour the render window

The first version of the bespoke harness computed RMS over the whole buffer,
including the startup transient, while `-skip` affected only the printed dump.
On a strongly attenuated signal the transient dominated, and the resulting
"discrepancy" against theory was 5x at 8 kHz — an hour spent suspecting the
filter before suspecting the measurement.

`--skip` must apply to reductions and dump alike, and `--reduce` should report
the window it used, so the output is self-describing.

## 8. Open questions

- **Where it lives.** A third binary in `crates/cranelift-ffi/src/bin/`
  alongside `impulse_cranelift`, or its own `crates/faust-probe` with the
  Cranelift crate as a dependency. The latter is better if the tool is ever to
  gain an interpreter backend for comparison (`--backend cranelift|interp`),
  which §6.1 arguably wants.
- **Whether it should replace the two existing runners** once P2 is byte-exact,
  or coexist. Replacing removes a maintenance surface; coexisting keeps the
  regression path independent of a tool that is also under development.
- **`-vec` and scheduling flags.** `impulse-runner` exposes `--vectorize`,
  `--vector-size`, `--scheduling-strategy`. Whether probing needs them, or
  whether vector-mode verification stays with the impulse corpus, is not
  settled here.
