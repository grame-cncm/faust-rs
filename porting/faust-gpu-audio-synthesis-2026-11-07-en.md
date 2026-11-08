# Audio on the GPU with Faust: does it make sense? A synthesis

Date: 2026-11-07

This document summarises a day of measurements, mostly on generated Mojo code
running on a GPU, and what they say about which audio algorithms belong on a
GPU and which do not.

- **Machine of record:** Apple M5, with 4 performance cores, 6 efficiency
  cores and 32 GB of unified memory. GPU through Metal.
- **Toolchain:** Mojo `1.3.0.dev2026110705` with `modular`
  `26.8.0.dev2026110705`.
- **Code under test:** faust-rs `-lang mojo` output. Everything is computed in
  `f32`, because the Apple GPU has no `f64`.

The detailed records are listed in §8. All numbers below were measured on that
machine, and nothing here was measured on another GPU.

## 1. The short answer

**Yes, for one shape of problem: thousands of independent instances of the
same Faust program.** An instance can be a voice, a partial, a mode, a
channel, a grain or an element of a training batch. The shape works best with
these conditions:

- blocks of 128 to 256 frames or more;
- a few tens of milliseconds of output latency are acceptable;
- `f32` is precise enough.

When all of that holds, the measured gains over the CPU on the same machine
are:

- **×2 to ×5 against the 4 to 10 CPU threads;**
- **×7 to ×18 against one core.**

One program was found that only the GPU can play on this machine: an
additive choir of 48,000 partials, each a sine with vibrato, breath and a
resonant noise band. It needs 2.1 times the whole CPU (LLVM code on all ten
cores) and runs on the GPU at two thirds of its real-time budget (§5).

These gains are against the same program run one instance per voice on the
CPU. For voices as small as one mode, a CPU program would rather group them
(50 modes in one bell, a bank of modes in one program), and that is a much
stronger baseline: against it, the carillon of §5 gains **×1.15 to ×1.9**,
from about 11,000 modes.

**No, for everything else**, in particular:

- **one instance of a program, however heavy.** A single reverb runs 30 to 100
  times slower on the GPU than on one CPU core;
- **ordinary polyphony.** Tens to a few hundred voices sit below the crossover,
  which is about 1,000 voices for small programs and about 4,000 for large ones
  (about 10,000 for modes grouped in a CPU program, §5);
- **very small blocks (64 frames and less):** a fixed cost of about 0.2 ms per
  block makes them fail;
- **`f64` work**, and programs whose result is very sensitive to rounding;
- **parallelising the inside of one program** (its loop graph). The C++
  pull request's `-gpu` path does this, and it measured 270 to 2,600 times
  slower than the CPU.

**What Faust brings.** The scalar `compute` of one instance runs unchanged as
the body of a GPU thread. The compiler's real work is the **state layout**:
each state field must be stored for all voices side by side, a "structure of
arrays", rather than one structure per voice. Without that layout, programs
with delay lines lose on the GPU. With it, they gain ×20 to ×31 and pull ahead
of the CPU.

## 2. What was measured

### 2.1 Two ways to use a GPU

| approach | parallel axis | result |
|---|---|---|
| `-gpu` of grame-cncm/faust#1293 | the sub-loops of one instance, as tasks | 270 to 2,600 times slower than the CPU: `APF` runs on 1 GPU thread, each block costs several kernel launches |
| one GPU thread per instance (faust-rs, this plan) | the instances | viable from about 1,000 instances (§2.3) |

A GPU wants thousands of threads running the same code without depending on
each other. One Faust instance does not offer that: its sub-loops are few and
ordered, and its recursions order the samples in time. Many instances do
offer it, and nothing in the generated code has to change for it.

### 2.2 The fixed cost of a block

An empty kernel plus its synchronisation costs **about 215 µs** per block,
whatever the number of voices. Copying inputs and outputs adds to that, up to
4.4 ms for 65,536 voices of 256 frames.

The real-time budget of one block is:

| block | at 48 kHz | at 44.1 kHz |
|---|---|---|
| 256 frames | 5.33 ms | 5.8 ms |
| 64 frames | 1.33 ms | 1.45 ms |

So the floor takes 4 % of a 256-frame block and 16 % of a 64-frame block.
With the per-block control and mix kernels added, a 64-frame block used about
0.7 ms in the real-time runner, and it under-ran.

### 2.3 When the GPU beats the CPU threads

The CPU baseline is the faster of 4 and 10 threads. 10 threads were rarely
faster, because the 6 extra cores are efficiency cores. Block size is 256
frames.

| program | state per voice | GPU faster from | gain at 4,096 voices (vs CPU threads) |
|---|---|---|---|
| `APF` (biquad all-pass) | 32 B | 1,024 voices | ×3.6 |
| additive partial (sine oscillator) | 24 B | 4,096 partials | ×2.4 (×4.1 at 65,536) |
| modal resonator (two-pole) | 40 B | 1,024 modes | ×2.8 |
| `osc` (table sine; table shared) | 32 B | 1,024 voices | ×4.8 |
| `karplus32` (32 strings) | 482 KB | 1,024 voices | ×4.8 |
| `freeverb` | 150 KB | 4,096 voices | ×2.2 |
| `zita_rev1` | 480 KB | 4,096 voices | ×2.2 |

The CPU side of this table runs one instance per voice. That is a fair
baseline for a voice that does real work per sample (a reverb, 32 strings, a
sine), and a weak one for a voice as small as one resonator: a CPU program
would put many of them in one `compute`, which is 4 to 7 times cheaper (§5,
modal synthesis and the carillon).

The last three rows (large state) depend on the structure-of-arrays layout.
With one structure per voice they never beat the CPU, and they got worse as
voices were added: `freeverb` was 14 times slower than the CPU at 4,096
voices. Each voice walking its own 150 KB defeats the GPU caches. Laying each
state element out for all voices side by side gives:

| program | layout gain at 1,024 voices | layout gain at 4,096 voices |
|---|---|---|
| `freeverb` | ×2.8 | ×31 |
| `karplus32` | ×4.5 | ×26 |
| `zita_rev1` | ×1.9 | ×20 |

Small numbers of voices go the other way: at 64 to 256 voices the layout is
mostly slower (×0.6 to ×0.9).

### 2.4 A single instance

One instance on the GPU runs on one thread, sequentially, at a fraction of a
CPU core's speed:

| program | 1 instance on 1 CPU core | 1 instance on the GPU |
|---|---|---|
| `freeverb` | 14 µs | 0.56 to 1.5 ms |
| `zita_rev1` | 30 µs | 1.0 to 2.6 ms |
| `karplus32` | 37 µs | 1.2 to 3.2 ms |

These are times per 256-frame block. **The GPU is not a faster processor for
one DSP. It is many slow ones.**

### 2.5 Real time

A real-time runner was built: 256-frame blocks at 48 kHz, written through
PortAudio's blocking API, with voices driven by scripted notes.

- It plays **256 voices for 30 s without an underrun**, and up to 16,384 voices
  for 10 s.
- The output latency PortAudio grants is **43 ms** at 256 frames, 30 ms at 128,
  and 23 to 29 ms at 64. The 64-frame block under-runs.
- The blocking API adds buffering on top of CoreAudio. A runner driven by the
  audio callback would get closer to the block period; it has not been written.

### 2.6 Correctness

- The structure-of-arrays layout changes no arithmetic. On the CPU, through
  the interpreter, every voice is bit-identical to an independent scalar
  instance.
- On the GPU, about 120 of 131 test programs match the same program on the
  CPU to within the last ulp (relative difference 1e-5 or less).
- Eight programs drift apart. The difference starts at rounding level (the
  GPU and CPU compilers generate different floating-point code, and the GPU
  flushes denormals to zero), and feedback amplifies it, up to 45 % for
  `phaser_flanger`. The GPU math functions themselves are accurate to about
  1e-6.

## 3. What speaks for it

- **No new DSP code.** A Faust program written for one voice runs N times as
  it is. The compiler handles the state layout; nothing else changes.
- **Throughput at scale.** Between ×2 and ×5 the throughput of 4 to 10 CPU
  threads, and ×7 to ×18 that of one core, against one instance per voice on
  the CPU; ×1.15 to ×1.9 against a CPU program that groups small voices (the
  carillon, §5).
- **It frees the CPU.** Even where the GPU does not play more voices than the
  CPU threads (`APF` on the grid measured), moving them to the GPU leaves the
  cores to the rest of the application.
- **Shared tables are free.** A generated table (`osc`'s 256 KB sine) is stored
  once for all voices, not per voice. In the structure-of-arrays form `osc`
  needs 32 bytes per voice and plays 65,536 voices within the real-time budget.
- **Per-voice everything.** Each voice has its own controls, state and
  lifecycle (a voice can be reset alone), which is what polyphony and
  training batches need.
- **Offline work has no latency constraint.** Parameter sweeps, calibration
  batches and dataset rendering can use very large N and long blocks, where
  the GPU is at its best.

## 4. What speaks against it

- **The crossover is high.** Below about 1,000 voices for small programs, and
  about 4,000 for large ones, the CPU threads win. When the voices are tiny
  and a CPU program can group them (modes), it is about 10,000. Most musical
  uses of Faust sit far below that.
- **The fixed cost per block and the latency.** About 0.2 ms per block, plus
  copies and synchronisation, excludes small blocks. The real-time runner
  needed 30 to 43 ms of output latency. This is fine for a sound installation
  or a renderer, and not for live monitoring.
- **`f32` only on consumer GPUs** (`f64` is very slow or absent). Recursive
  filters at high Q or low frequency, long accumulations, and feedback
  networks sensitive to rounding drift from their CPU results.
- **Divergence.** All threads of a SIMD group execute the same instruction. A
  `select2` lowered as a branch, `ondemand` or `iterate` blocks whose rate
  differs per voice, or voices in different states make threads wait for each
  other. The result stays correct; the speed drops.
- **Memory.** State is multiplied by N. 4,096 voices of `zita_rev1` need 2 GB,
  and the 32-bit indices of the layout cap `karplus32` at about 17,800 voices.
- **An immature toolchain.** Mojo changes from one release to the next, and
  this work met three failures in its GPU path, two of them silent, each of
  which a harness now guards against:
  1. a pointer into a thread's private memory loses its writes through a call;
  2. a buffer reached through a pointer stored in another buffer, rather than
     passed as an argument, reads zeros;
  3. a compiler hang on nested loops (fixed in Mojo 1.3).
  The whole path is opt-in, needs a 1.5 GB toolchain, and is outside CI.
- **The CPU baseline is not the strongest possible.** Most CPU numbers run
  one voice per call, without SIMD across voices. For tiny voices, grouping
  them in one CPU program was measured on the carillon, and it cut the GPU's
  gain from what one call per voice suggested (×5.7 to ×9.7 against the batch
  code on the CPU) to ×1.15 to ×1.9. SIMD across voices, which the
  structure-of-arrays layout allows, would narrow the gap further, possibly
  to nothing for small programs. This has not been measured, and it is the
  most important caveat on the ratios above.
- **Not measured:** power and heat; a GPU shared with graphics (inside a DAW
  or with a visual front end); discrete GPUs, where copies cross a bus rather
  than unified memory.

## 5. Algorithm by algorithm

### Additive synthesis: a good fit at scale (measured)

Each partial is one instance of a small oscillator program with its own
frequency and gain (`tests/corpus/gpu_partial_sine.dsp`). Measured at 256
frames with the bench harness, which copies every partial's output back to
the host:

| partials | GPU | CPU threads | GPU vs CPU |
|---|---|---|---|
| 1,024 | 0.28 ms | 0.22 ms | ×0.8 |
| 4,096 | 0.33 ms | 0.79 ms | ×2.4 |
| 16,384 | 0.90 ms | 3.2 ms | ×3.5 |
| 65,536 | 3.1 ms | 12.7 ms | ×4.1 |

65,536 independently controlled partials fit in one 256-frame block, where the
CPU threads stop between 25,000 and 30,000. These GPU times are conservative:
at 65,536 partials the copies of all the partials' outputs take 2.3 ms of the
3.1 ms, and the kernel about 1.0 ms. An instrument sums the partials on the
device and copies only the mix, as the real-time runner does (§2.5).

Two qualifications:

- Writing the partials as `par(i, N, osc)` inside **one** Faust program gives
  one instance, so no GPU parallelism. The partials must be instances.
- For pure sinusoids with smooth control, inverse-FFT additive synthesis
  ("FFT⁻¹") is far cheaper on the CPU than any oscillator bank. The
  oscillator-bank GPU path wins when each partial is a real signal processor
  (its own envelope, noise, filter, nonlinearity), which the FFT method cannot
  express.

### A case only the GPU can play: an additive choir (measured, 2026-11-08)

This is the shape the qualification above points to, measured end to end:
every partial is a real signal processor, and there are tens of thousands of
them.

- **One partial** (`tests/corpus/gpu_choir_partial.dsp`, self-contained):
  - a sine with its own vibrato;
  - a slow amplitude swell (breath);
  - a band of noise centred on the partial, through a TPT state-variable
    band-pass whose centre follows the vibrato sample by sample.

  Per sample that is three sines, a tangent, a linear congruential noise and
  the filter. An inverse-FFT resynthesis cannot express a resonant noise band
  per partial that moves with a per-sample vibrato.
- **The scene** (`tests/impulse-tests/archs/choirmojogpubatch.mojo`):
  - S singers in four parts, each singer the harmonics of its part's lowest
    note up to 20 kHz, every harmonic one instance;
  - every singer with its own detuning (up to 15 cents), vibrato, breath and
    place in the stereo field;
  - the gains shaped by the five formants of the vowel "a" for the part (the
    Csound formant table) over a source tilt, with a noise share that rises
    with frequency;
  - four chords (D minor, B flat, F, C), 5 s each, looped; a chord change
    moves every partial's frequency and gain, and the program glides the
    gains.

  448 singers give **48,048 partials**.
- **Validation.** 8 singers (858 partials) for 12 s, across two chord
  changes: the GPU mix equals the same choir of ordinary scalar instances on
  the CPU to **1.2e-4 relative**, sample by sample.

The CPU baseline is the best code Faust produces for this voice on this
machine: one LLVM-compiled program of 256 distinct partials, scalar (the
vector modes are slower here, the recursions dominate), **7.4 ns per partial
and per sample** on one core. Ten copies run at once, one per core, reach
**1.09e9 partial-samples per second**. That is the whole machine's ceiling
for this voice: about **22,600 partials in real time** at 48 kHz, with no mix,
no operating system and no headroom.

The GPU computes a partial in about 0.23 ns per sample, some 32 cores' worth.
Measured at 256 frames and 48 kHz (median block, mix included):

| singers | partials | GPU | GPU, % of budget | whole CPU (LLVM, 10 cores), % of real time |
|---|---|---|---|---|
| 32 | 3,432 | 0.81 ms | 15 % | 15 % |
| 128 | 13,728 | 1.29 ms | 24 % | 61 % |
| 256 | 27,456 | 2.22 ms | 42 % | 121 % |
| 448 | 48,048 | 3.61 ms | 68 % | 212 % |
| 640 | 68,640 | 4.93 ms | 93 % | 303 % |

The CPU column is the measured throughput of the whole machine, so it is an
upper bound for any real CPU player of the same code: it would add a mix and
the scheduling of threads. In real time (PortAudio, 43 ms latency, muted):

- **448 singers, 48,048 partials:** 20 s with no underrun; median block
  3.66 ms. The worst blocks (13.6 ms) are the chord changes, where the host
  recomputes and uploads three controls of every partial; the latency
  absorbs them.
- **576 singers, 61,776 partials:** 20 s with no underrun; median block
  4.48 ms, 84 % of the budget.

The 448-singer choir needs **2.1 times the whole CPU** and runs on the GPU at
two thirds of its budget. On this machine it is a GPU-only program.

The limits of that statement:

- **A CPU version vectorized by hand across partials was not measured.** With
  four `f32` lanes and vector sine and tangent approximations it might reach
  three to four times the scalar throughput, close to the GPU's. Faust does
  not emit such code (its `-vec` is vectorized over time, and slower here).
- The harness's own CPU baseline, the generated Mojo scalar code on 1, 4 and
  10 threads, runs this voice at about 38 ns per partial and per sample, five
  times slower than LLVM, so it is not the reference. LLVM is.

Two lessons from building it:

- **A one-pole glide in `f32` stops short of its target.** `y = x(1 - c) +
  c·y'` stalls once the step `x(1 - c)` is under an ulp, at up to an ulp over
  `1 - c` from the target: 2.3e-4 relative with an 80 ms time constant at
  48 kHz, about 0.4 cent. A device that fuses the multiply-add and one that
  does not stall at different points, and a partial's phase then drifts apart
  by about a radian per second. The first version glided `freq` and failed
  validation that way. The voice now glides only `gain` and lets `freq` jump
  with a continuous phase.
- **The voice found a defect in the batch interpreter.** It is the first
  corpus voice that reads the sample rate, and the CPU differential of the
  batch layout (`crates/compiler/tests/batch_voices_interp.rs`) failed on it.
  A batch module moves `fSampleRate` into an arena. The `sample_rate`
  argument then read a heap slot the interpreter never wrote, so every voice
  ran at a rate of 1. The Mojo output was not affected, since it reads the
  argument directly. Fixed in `crates/codegen/src/backends/interp/mod.rs`,
  with a sample-rate program added to the differential.

**A hall, on the CPU.** `make mojo-gpu-choir-reverb` (or `play ... reverb`)
passes the stereo mix through `tests/corpus/gpu_choir_reverb.dsp`,
`re.zita_rev1_stereo` mixed with the dry signal (dry 0.4, wet 2, T60 5 s by
default: at steady state `zita_rev1` returns about 0.29 of its input, so the
hall sits about 3 dB above the choir). The reverb is one instance, the shape
a GPU runs 30 to 100 times slower than one core (§2.4), so it runs on the
CPU, in the playback loop: **43 to 53 µs per block**, about 1 % of the
budget, against 1 to 2.6 ms for one `zita_rev1` on the GPU. With it, 448 singers still play 20 s with no
underrun. That version also sings a wider choir (registers from C2 to G5,
per-part vibrato, breathiness and stage placement, harmonics up to 16 kHz so
that 448 singers stay at 46,592 partials, about 3.5 ms per block) and eight
chords (D minor, B flat, F, C, then G minor 7, the Neapolitan E flat, A7 and
D major), each followed by 3 s of rest so that the hall's tail is heard. The work splits as the rest of this document says it should: the
GPU computes and sums the 48,048 partials, the CPU runs the single program
on the two channels that come back.

The lanes are `make -C tests/impulse-tests mojo-gpu-choir CHOIR=448` (it
validates, times and plays the choir dry) and `mojo-gpu-choir-reverb` (it
validates and plays the choir in the hall); both play muted unless
`POLY_AUDIBLE=audible`.

### Modal synthesis: a fit only at very large scale (measured)

Each mode is a resonator instance (frequency, T60, gain;
`tests/corpus/gpu_mode_resonator.dsp`). All modes are fed the same
excitation, broadcast as their input. Measured as for the partials, with
every mode's output copied back:

| modes | GPU | CPU threads | GPU vs CPU |
|---|---|---|---|
| 1,024 | 0.22 ms | 0.30 ms | ×1.4 |
| 4,096 | 0.38 ms | 1.06 ms | ×2.8 |
| 16,384 | 1.6 ms | 4.3 ms | ×2.7 |
| 65,536 | 6.3 ms | 17.4 ms | ×2.7 |

The number of modes is not in the Faust program, which describes one mode:
it is the voice count the host chooses at run time (`BENCH_VOICES`, passed
to the struct's constructor and launched as one GPU thread per mode). In
these runs every mode kept the default parameters (440 Hz, T60 of 1 s): the
cost does not depend on the values, but the table measures N resonators,
not a tuned modal model, which would set each mode's frequency, decay and
gain with `set_control`, as the real-time runner sets each voice's
frequency. The same holds for the partials above.

At 65,536 modes the copies take 5.0 ms and the kernel 1.9 ms: summed on the
device, the bank would fit in a 256-frame block where the table shows it
just over.

**The CPU column of this table is weak.** It runs one instance per mode, one
call each, and a resonator is only a few operations per sample, so the calls
dominate. A CPU program would hold the modes in one program. Written so
(`par(i, N, mode(i))` with its own controls per mode, summed) and compiled with
`faustbench-llvm`, the bank costs, on **one** core:

| modes | GPU (above) | CPU threads (above) | one program, 1 core (LLVM) |
|---|---|---|---|
| 1,024 | 0.22 ms | 0.30 ms | 0.18 ms |
| 4,096 | 0.38 ms | 1.06 ms | 1.25 ms |

One core already beats the GPU at 1,024 modes, and a few cores would at
4,096. The carillon below measures this baseline in the same process, with
threads: the GPU wins from about 11,000 modes. The same check on the additive
partials finds a smaller bias, because the sine dominates each partial: one
program of 1,024 partials takes 0.53 ms on one core, against 0.22 ms on the
threads one instance at a time, and 4,096 take 4.5 ms.

This suits very large modal models (rooms or plates described by tens of
thousands of modes) and large scenes of modal objects. The excitation input and the output
sum are the parts to keep on the device: a summing kernel did this in the
runner. In `f32`, high-Q, low-frequency modes are the precision risk, because
their poles sit very close to the unit circle.

### A real modal use case: a carillon (measured)

The bank above uses identical, untuned resonators. A real modal model gives
every mode its own parameters. It was built from `pm.standardBell` of
physmodels.lib (the model behind `examples/physicalModeling/standardBell.dsp`):
a `pm.strikeModel` strike drives 50 `pm.modeFilter` modes, whose frequencies,
decays and gains `mesh2faust` derived from a finite-element model of a church
bell, with 7 strike positions.

- **Why many bells.** One bell is 50 modes, far below the crossover: on the
  GPU it costs 0.6 ms per block against 0.013 ms on one CPU core. The GPU use
  case is a **carillon or a bell installation**: hundreds to thousands of
  bells, each with its own size, strike position and decay, hence thousands
  of modes that all have different parameters.
- **Two Faust voices, written with the library itself**:
  - `tests/corpus/gpu_bell_strike.dsp`: one strike per bell;
  - `tests/corpus/gpu_bell_mode.dsp`: one mode, `pm.modeFilter(freq, t60,
    gain)`, the state-variable realisation the library uses so that the
    poles survive in `f32`. The direct-form resonator of the bank above is the
    form that drifted by up to 5 % on a bell, and a GPU computes in `f32`.
- **The parameters of every mode** come from the installed physmodels.lib
  (`tests/impulse-tests/tools/bell_modes.py` reads the tables at build time,
  so the repository holds no copy of them):
  - bell b of B: frequencies scaled by 2^(semitones/12), from an octave below
    the model to two octaves above;
  - T60 by the library's formula, divided by the scale (a small bell rings
    shorter);
  - the gain row of strike position b % 7;
  - modes above 0.45 times the sample rate muted.
- **Per block on the GPU**:
  - the strikes run (one instance per bell);
  - the 50·B modes run, each reading its own bell's strike output directly:
    its channel table points into the strike data, so nothing is copied;
  - a two-stage mix sums each bell's modes, then the panned bells;
  - only the stereo mix returns to the host.
- **Validation.** One bell on the GPU (one strike instance and 50 mode
  instances, summed and divided by 50) reproduces the original
  `pm.standardBell`, computed as one program on the CPU, to **1.5e-4
  relative** for every strike position over 2 s. The difference comes from
  the mode parameters, computed in `f64` on the host where the original
  computes them in `f32`, and from the order of summation.

Measured at 256 frames and 48 kHz, as the median block, with the same score
and the same panned mix on both sides. The CPU baseline is the program a CPU
user would write: one ordinary instance of `pm.standardBell` per bell (the
scalar contract, its 50 modes in one `compute`), the bells split over 1, 4 or
10 threads. The last CPU column runs the GPU's batch code on the CPU instead:

| bells | modes | GPU | CPU, 1 thread | CPU, 4 threads | batch code, 4 CPU threads | GPU vs best CPU |
|---|---|---|---|---|---|---|
| 1 | 50 | 0.62 ms | 0.013 ms | 0.05 ms | 0.06 ms | ×0.02 |
| 20 | 1,000 | 0.62 ms | 0.25 ms | 0.084 ms | 0.59 ms | ×0.14 |
| 37 | 1,850 | 0.64 ms | 0.47 ms | 0.15 ms | 0.85 ms | ×0.24 |
| 64 | 3,200 | 0.67 ms | 0.84 ms | 0.24 ms | 1.23 ms | ×0.36 |
| 256 | 12,800 | 0.84 ms | 3.47 ms | 0.97 ms | 4.87 ms | ×1.15 |
| 1,024 | 51,200 | 2.0 ms | 13.7 ms | 3.88 ms | 19.6 ms | ×1.9 |

- The 10 threads were slower than the 4 at every size.
- The one-thread column agrees with LLVM: `faustbench-llvm` puts one
  `pm.standardBell` at 0.23 % of a core at 48 kHz, and 37 distinct bells in
  one program at 8.2 %, or 0.44 ms per block.
- **The GPU overtakes the CPU threads at about 220 bells (11,000 modes)**, and
  gains ×1.9 at 1,024. Below that, its fixed cost of about 0.6 ms per block
  (six kernel launches, three copies and a synchronisation) is more than the
  whole work: by the slope of the table, 37 bells are about 50 µs of GPU
  computation.

In real time (PortAudio, 43 ms latency, muted), 64, 256 and 1,024 bells play
10 s without an underrun. A block then takes about 2.7 ms whatever the size:
paced by the audio output, the GPU idles between blocks and its clock drops.
The two tunes (37 bells) take a median of 2.3 to 2.4 ms per block in real
time, 43 to 46 % of the budget, and their worst blocks reach 10.7 ms; the
latency absorbs them. The same 37 bells cost 0.47 ms on one CPU core, 9 % of
the budget. On the CPU threads, 1,024 bells take 73 % of the budget in the
bench (not run in real time), so four cores could play about 1,400 bells;
the GPU's 2.0 ms at 1,024 suggests about twice that. Neither limit was run.

Two lessons from building it:

- **The reduction must be parallel.** The first mix gave one thread per frame
  and walked every mode: 256 threads summing up to 51,200 outputs each. It
  was 10 to 16 times slower than the two-stage mix that replaced it: one
  thread per bell and frame, then one thread per frame over the bells.
  Summing voices is part of the GPU work, and its shape matters as much as
  the voices'.
- **The baseline must be the program a CPU user would write.** The first
  version of this section compared the GPU with its own batch code run on the
  CPU, and reported ×5.7 to ×9.7. That code is 4 to 5 times slower on a CPU
  than ordinary instances of `pm.standardBell` (table above): its layout,
  made for the GPU, keeps each voice's state N elements apart, and every mode
  is a call of its own. Against the ordinary program the gain is ×1.15 to
  ×1.9.
- **Composition stays on the device.** Two batch programs, strikes and
  modes, are chained by pointing one's inputs at the other's outputs. Faust
  describes each voice; the host wires the voices into a scene.

The lane is `make -C tests/impulse-tests mojo-gpu-bells BELLS=256`
(`archs/bellsmojogpubatch.mojo`). It validates, times, and plays muted;
`bench` prints the GPU and both CPU baselines. `POLY_AUDIBLE=audible` plays
the carillon aloud. `BELLS_SCORE` chooses the
score: `cloud` (every bell, staggered), or two tunes on 37 bells, one per
semitone: `melody` (Beethoven's "Ode to Joy") and `chime` (the Westminster
Quarters, then a low hour bell). Both play with no underrun.

### Filter banks, vocoders, analysis bands: a good fit beyond about 1,000 bands

This is the modal structure again: many band filters on one input, each band
with its own parameters. A 32-band vocoder is far below the crossover; a
4,096-band analyser or resynthesiser is above it.

### Reverberation: a poor fit, except as many reverbs

- **A single reverb** (Schroeder, Freeverb, FDN, `zita_rev1`): measured 30 to
  100 times slower on the GPU than on one CPU core. Its recursion orders the
  samples and its few delay lines give no parallel axis.
- **Many independent reverbs:** 4,096 `freeverb`s run ×2.2 faster than on the
  CPU threads, using the structure-of-arrays layout. That is a rare need
  (per-source reverberation in a large spatial scene, an offline batch of room
  variants).
- **Convolution reverb** is GPU-friendly (partitioned FFT convolution), but
  Faust's sample-by-sample signal model does not produce it. It would take FFT
  or matrix primitives that Faust does not emit today.
- **A large FDN with a dense mixing matrix** is a matrix-vector product per
  sample, inside one instance. It is parallel only through the matrix, which
  Faust lowers as scalar sums, not as a matrix kernel.

### Physical modelling: depends on where the parallelism is

- **Many strings, tubes or waveguides** (instances): good. `karplus32`, with 32
  strings per instance, runs ×4.8 faster than the CPU threads at 4,096
  instances. A piano with sympathetic strings (about 230 strings) is still
  below the crossover.
- **One large mesh** (2D/3D waveguide, FDTD membrane or plate, mass-spring
  network): the parallelism is in the mesh nodes, inside one instance, coupled
  at every sample. Faust writes it as one program, so the approach here does
  not help. Such a model wants a stencil kernel over a grid, which is outside
  Faust's signal model.

### Polyphonic instruments: usually below the crossover

Typical polyphony (16 to 128 voices) is well served by the CPU. The GPU
becomes relevant for **massive polyphony**: swarms, clouds, crowds,
thousands of simultaneous voices.

### Granular synthesis: a good fit when grains number in the thousands

Each grain is an instance with its own position, pitch and envelope. Starting
and stopping grains is per-voice control, and silent grains cost little
divergence when grains are grouped. Below about 1,000 simultaneous grains, the
CPU wins.

### Spatialisation and large loudspeaker arrays: a good fit at hundreds of channels

- Wave-field synthesis or speaker-array rendering (hundreds of channels, each
  running the same per-channel delay, gain and filter program) is a natural
  instance axis.
- Higher-order ambisonic decoding with many speakers, and the mixing of many
  sources to many outputs, is a matrix product. It is better written as a
  matrix kernel than as Faust instances.

### Spectral and frame-rate processing: a poor fit as written

An FFT inside one Faust program (faust-rs `ondemand` FFTs) is computed within
one instance, so only one thread works on it. Many simultaneous STFT channels
would form an instance axis. A GPU FFT library would do one large transform far
better than a Faust signal graph.

### Learning, calibration, dataset rendering: the best fit

These run offline, with very large N and long blocks:

- multistart and grid searches;
- evolution-strategy populations;
- calibration of a differentiable DSP against many examples;
- rendering thousands of parameter variants of a sound.

There is no latency constraint, the fixed cost per block is amortised, and
the structure-of-arrays layout keeps memory access efficient. This is also
where the `fad`/`rad` differentiation of faust-rs meets the batch axis: one
tangent or one gradient per instance.

The limits are `f32` precision for gradients, and the memory of `rad` tapes,
which grows as N × block × tape width.

## 6. A decision guide

| question | GPU worth trying | stay on the CPU |
|---|---|---|
| How many independent instances? | thousands or more | one, or a few hundred |
| Where is the parallelism? | across instances | inside one program (its loop graph, a mesh, a matrix) |
| State per instance | any, with the structure-of-arrays layout | (a large state without that layout loses) |
| Block size and latency | 128 to 256 frames or more; 30 to 45 ms acceptable, or offline | 64 frames or less; live monitoring |
| Precision | `f32` is enough | needs `f64`, or the feedback is rounding-sensitive |
| Control flow | the same path for all voices | rates, branches or iteration counts differ per voice |
| Another algorithm available? | — | inverse-FFT additive, FFT convolution, matrix kernels |

## 7. What would come next

- **SIMD across voices on the CPU.** The structure-of-arrays layout lets a
  CPU process 4 to 16 voices per instruction. This would give the fair
  baseline, and probably a CPU win of its own for small programs (§4). It is
  the first thing to measure. The grouped-program baseline was measured for
  the carillon only; the §2.3 grid still runs one instance per voice.
- **A real-time runner driven by the audio callback**, to bring the latency
  from 43 ms towards the block period.
- **Inputs and mixes as device-side primitives:** broadcast inputs, sums by
  group of voices, and the matrix mixes of spatial rendering. These are
  written by hand in the harnesses today.
- **Constants shared across voices** (`fConst*`, the sample rate), to cut state
  reads.
- **A time axis for the parts of a program without recursion.** Each instance
  would become a grid of instances × samples, which raises the parallelism
  of programs that are cheap per sample.
- **Other targets:** WGSL through `wgpu` (portable), and CUDA (with `f64` on
  data-centre parts) for what Metal cannot run.
- **Upstream reports** of the two Mojo GPU failures found (§4), once reduced to
  minimal reproducers.

## 8. Records

- `porting/mojo-gpu-pr-1293-analysis-and-batch-axis-2026-09-18-en.md`: the
  first analysis and the instance axis.
- `porting/mojo-gpu-batch-of-voices-plan-2026-11-07-en.md`: the plan and the
  outcomes of G1 (correctness), G2 (cost model) and G4 (real time).
- `porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`: the
  structure-of-arrays layout (`fir::batch`), its checker, the CPU
  differential, and the Mojo batch contract with its measurements.
- `porting/mojo-backend-port-analysis-and-plan-2026-11-06-en.md`: the Mojo
  backend.
- Lanes in `tests/impulse-tests`:
  - `make mojo-gpu` and `make mojo-gpu-batch` (correctness);
  - `make mojo-gpu-bench` and `make mojo-gpu-bench-batch` (cost);
  - `make mojo-gpu-poly` (real time).
- The additive choir of §5: `tests/corpus/gpu_choir_partial.dsp`;
  `make mojo-gpu-choir`.
- The carillon of §5: `tests/corpus/gpu_bell_strike.dsp`,
  `gpu_bell_mode.dsp` and the reference `gpu_bell_reference.dsp` (the
  original `pm.standardBell`); `make mojo-gpu-bells`.
- The GPU voices measured for §2.5 and §5 are in the golden corpus:
  - `tests/corpus/gpu_voice_saw.dsp`, the real-time runner's instrument;
  - `tests/corpus/gpu_partial_sine.dsp`, the additive partial;
  - `tests/corpus/gpu_mode_resonator.dsp`, the modal resonator.
  `crates/compiler/tests/batch_voices_interp.rs` checks all three in CI: every
  voice of their batch layout is bit-identical to an independent scalar
  instance. The bank figures are reproduced, within 15 %, by
  `make -C tests/impulse-tests mojo-gpu-bench-batch BENCH_DSPDIR=../corpus
  BENCH_PROGRAMS="gpu_partial_sine gpu_mode_resonator" BENCH_NAME=banks`
  (and `mojo-gpu-bench` with the same variables for the CPU columns).
