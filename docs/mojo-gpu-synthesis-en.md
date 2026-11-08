---
title: "Faust on the GPU with the Mojo backend: a gentle synthesis"
date: 2026-11-08
---

**Date:** 2026-11-08

**Audience:** a curious reader, a Faust user or a musician who wonders what a
graphics card can do for sound. No knowledge of GPUs or compilers is assumed.

French version: [mojo-gpu-synthesis-fr.md](mojo-gpu-synthesis-fr.md) (same
content; keep both versions synchronized).

**Reference documents:**

- [`faust-gpu-audio-synthesis-2026-11-07-en.md`](../porting/faust-gpu-audio-synthesis-2026-11-07-en.md),
  the measurements and the detailed verdict;
- [`mojo-backend-port-analysis-and-plan-2026-11-06-en.md`](../porting/mojo-backend-port-analysis-and-plan-2026-11-06-en.md),
  the Mojo backend;
- [`mojo-gpu-batch-of-voices-plan-2026-11-07-en.md`](../porting/mojo-gpu-batch-of-voices-plan-2026-11-07-en.md)
  and
  [`mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`](../porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md),
  the GPU work;
- [`batch-state-register-promotion-plan-2026-11-08-en.md`](../porting/batch-state-register-promotion-plan-2026-11-08-en.md),
  the state of the copies kept in local variables (§5);
- DIFF-BACK-010 in
  [`faust-rs-vs-faust-cpp-differences-en.md`](../porting/faust-rs-vs-faust-cpp-differences-en.md),
  what differs from C++ Faust.

**Goal:** explain what the Mojo backend makes possible (running some Faust
programs on a GPU), how much computing power that brings compared with the
processor, and where the approach stops.

**Contents**

1. [What is this about?](#1-what-is-this-about) — a new output language for Faust.
2. [A CPU and a GPU](#2-a-cpu-and-a-gpu) — a few fast cooks or a thousand slow ones.
3. [Why one Faust program does not speed up](#3-why-one-faust-program-does-not-speed-up) — the time order of recursion.
4. [The idea that works: many copies of the same program](#4-the-idea-that-works-many-copies-of-the-same-program) — voices, partials, modes.
5. [What the compiler does](#5-what-the-compiler-does) — the state laid out for the GPU.
6. [What was measured](#6-what-was-measured) — from a slower GPU to a GPU-only program, then learning: finding the modes of a sound.
7. [CPU and GPU in the same program](#7-cpu-and-gpu-in-the-same-program) — the choir in a hall.
8. [Latency: the smallest audio buffer](#8-latency-the-smallest-audio-buffer) — why the GPU needs larger blocks than the CPU, and how PortAudio is driven.
9. [Trying it](#9-trying-it) — installing the toolchain, the commands, and writing your own GPU program.
10. [The current limits](#10-the-current-limits) — what the approach does not do, or not yet.
11. [Takeaways](#11-takeaways) — the one-page map.

## 1. What is this about?

Faust describes sound processing (an oscillator, a filter, a reverb, a whole
synthesizer) as a formula. The compiler turns that formula into code in
another language: C, C++, Rust, WebAssembly, and others.

`faust-rs` can now also write **Mojo**: `faust-rs -lang mojo foo.dsp`. Mojo is
a recent language, close to Python in its syntax, compiled to fast machine
code, and designed to run the same code on a processor (CPU) or on a graphics
card (GPU). The backend follows the contract of a Mojo backend proposed for
C++ Faust (pull request grame-cncm/faust#1293), so the code it writes plugs
into the same Mojo architecture files.

On a CPU, a Faust program in Mojo computes the same thing as in C++: the
backend passes the 132 programs of the impulse-response test suite. What is
new is that the same generated code can run **on the GPU**, provided you ask
the GPU for the right thing.

## 2. A CPU and a GPU

> **Definition: CPU and GPU.** A **CPU** has a few cores (10 on the machine measured here, an Apple M5),
> each very fast and very flexible. A **GPU** has thousands of small arithmetic
> units, each slower, grouped so that they execute the same instruction at the
> same time on different data.

An image helps. A CPU is a kitchen with a few excellent cooks: each one can
prepare any dish quickly, one after another. A GPU is a canteen with a
thousand cooks who are less skilled, but who all follow the same recipe at
the same moment. Ask the canteen for one gourmet dish and only one cook works,
slowly. Ask it for a thousand identical dishes and it is unbeatable.

The measurements say exactly this. One Faust reverb on the GPU runs **30 to
100 times slower** than on one CPU core:

| program | 1 copy on 1 CPU core | 1 copy on the GPU |
|---|---|---|
| `freeverb` | 14 µs | 0.56 to 1.5 ms |
| `zita_rev1` | 30 µs | 1.0 to 2.6 ms |

(times to compute a block of 256 samples, 5.3 ms of sound at 48 kHz.)

> **The point to remember.** The GPU is not a faster processor for one sound process. It is a very large
> number of slow ones. It only pays off when it has thousands of identical,
> independent tasks to do at once.

## 3. Why one Faust program does not speed up

The first idea is to split one program into pieces and give each piece to a
group of GPU units. The C++ proposal tried this (its `-gpu` option): it ran
**270 to 2,600 times slower** than the CPU.

The reason is in the nature of audio. Most sound processes have **memory**:
the output of a filter depends on its previous outputs, an echo on the
samples of the last second.

*A recursion: each sample needs the previous one*

```faust
_ : + ~ *(0.5)
```

Sample `n` cannot be computed before sample `n − 1`. Time imposes an order,
and that order cannot be split among a thousand cooks. Inside one program,
there are only a few independent pieces, and each costs a GPU launch.

## 4. The idea that works: many copies of the same program

The opening comes from elsewhere: many pieces of music need **thousands of
copies of the same small program**, each with its own settings, and the
copies do not depend on each other.

- **Additive synthesis**: a sound built from thousands of partials, each a
  small oscillator with its own frequency and amplitude.
- **Modal synthesis**: a bell, a plate or a room described by its resonant
  modes, each a small resonator.
- **Massive polyphony**: a cloud, a swarm, a crowd of voices.
- **Granular synthesis**: thousands of grains at once.
- **Offline work**: rendering thousands of variants of a sound, or training a
  differentiable model on many examples at once (§6.4 measures one case).

Here the canteen is in its element: each GPU unit runs **one copy** (an
*instance*) of the Faust program, all copies advance together, sample after
sample, and the time order is respected inside each copy.

> **Definition: Instance.** One running copy of a Faust program, with its own memory (its filter states,
> its delay lines) and its own controls. The Faust program describes **one**
> instance; how many instances run is chosen by the host program at run time,
> not written in the Faust code.

This has a pleasant consequence: **no new DSP code**. A Faust program written
for one voice runs N times as it is. The generated `compute` function of one
instance becomes, unchanged, the body of each GPU thread.

## 5. What the compiler does

If the Faust code does not change, what does the compiler do? It changes
**where the memory of each instance lives**.

By default, each instance keeps its memory in its own structure, like a
binder per musician. On a GPU that is a bad idea: at each instant, the thousand
units read the same variable of a thousand different binders, scattered
everywhere in memory, and the memory cannot keep up.

`faust-rs` can lay the memory out the other way, one drawer per variable,
with the values of all the instances side by side (a "structure of arrays").
When the thousand units read "their" variable, they read neighbouring values,
in one go.

```text
One structure per instance            Structure of arrays (for the GPU)

voice 0: state a, state b, state c    state a: voice 0, voice 1, voice 2, ...
voice 1: state a, state b, state c    state b: voice 0, voice 1, voice 2, ...
voice 2: state a, state b, state c    state c: voice 0, voice 1, voice 2, ...
```

For programs with a lot of memory, this changes everything. At 4,096 voices:

| program | memory per voice | gain from the layout |
|---|---|---|
| `freeverb` | 150 KB | ×31 |
| `karplus32` (32 strings) | 482 KB | ×26 |
| `zita_rev1` | 480 KB | ×20 |

Without this layout, these programs never beat the CPU; with it, they do.
The layout changes no arithmetic: on the CPU, every voice gives bit for bit
the same result as an ordinary copy of the program, and a test in the
continuous integration checks it.

**Keeping the state at hand.** With the memory well laid out, the generated
code still went to fetch it at every sample: read a filter's state, compute,
write it back, 48,000 times per second for each copy. That is a cook walking
back to the pantry for the salt at every move. A CPU keeps such values in
its registers without being asked. Mojo, on the GPU, does not, because it
cannot prove that the state memory and the output buffers do not overlap.

Since 8 November 2026 the compiler does it itself. `compute` reads the state
at the start of the block, keeps it in local variables for the whole loop,
and writes it back only at the end. Delay lines read at a varying position
stay in memory, because a circular buffer does not fit in registers. Again
no arithmetic changes: every voice stays bit for bit identical to an
ordinary copy, and an independent checker verifies each rewrite. The gain is
a factor of 2 to 5:

| program | state re-read at every sample | state kept local |
|---|---|---|
| carillon, 256 bells, one block | 0.84 ms | 0.46 ms |
| modal learning (§6.4), 1,024 modes, one epoch | 75 ms | 14.8 ms |
| modal learning, 16,384 modes, one epoch | 659 ms | 292 ms |

## 6. What was measured

All the figures come from one machine, an Apple M5 laptop (4 fast cores,
6 efficient cores, its integrated GPU), at 48 kHz, in blocks of 256 samples.
The budget of a block is **5.33 ms**: past it, the sound breaks up.

Sections 6.1 to 6.3 were measured before the compiler kept the state local
(§5). Their crossovers are therefore pessimistic: the GPU now wins earlier,
by how much has not been re-measured yet. Section 6.4 was measured after.

### 6.1 The crossover

Below some number of copies the CPU wins, above it the GPU wins. Against the
same program run one copy at a time on the CPU threads:

| program | GPU faster from | gain at 4,096 copies |
|---|---|---|
| biquad all-pass (`APF`) | 1,024 | ×3.6 |
| table oscillator (`osc`) | 1,024 | ×4.8 |
| `karplus32` (32 strings) | 1,024 | ×4.8 |
| `freeverb`, `zita_rev1` | 4,096 | ×2.2 |

The order of magnitude to keep: **a thousand copies for small programs, a
few thousand for large ones**. Ordinary polyphony (16 to 128 voices) stays
far below.

### 6.2 A carillon: a lesson in honesty

A real modal model: the church bell of `pm.standardBell` (physmodels.lib),
50 modes per bell, each mode its own GPU instance with its own frequency,
decay and gain. One GPU bell matches the original to 1.5e-4.

The first measurement compared the GPU with its own code run on the CPU, and
showed ×5.7 to ×9.7. But that code, laid out for the GPU, is 4 to 5 times
slower on a CPU than the program a CPU user would write (one ordinary
instance per bell, its 50 modes in one loop). Against that program:

| bells | modes | GPU | CPU, 4 threads |
|---|---|---|---|
| 37 | 1,850 | 0.64 ms | 0.15 ms |
| 256 | 12,800 | 0.84 ms | 0.97 ms |
| 1,024 | 51,200 | 2.0 ms | 3.9 ms |

The GPU only wins from about 220 bells, by ×1.9 at 1,024. A melody on 37
bells costs 9 % of one CPU core. **The comparison is only fair against the
best CPU version**, and getting that version right divided the gain by five.

This table predates the state kept local (§5). Re-measured once since, 256
bells take 0.46 ms on the GPU, against 1.14 ms for the CPU threads measured
in the same process: ×2.5 instead of ×1.15. The carillon therefore wins well
before 220 bells, but the new crossover has not been measured.

### 6.3 A choir only the GPU can play

To find a program that really needs the GPU, each copy must do real work. The
choir does: each **partial** (`tests/corpus/gpu_choir_partial.dsp`) is a sine
with its own vibrato, a breath swell, and a band of noise filtered around the
partial, whose centre follows the vibrato sample by sample. That is what makes
a voice breathy, and what a cheaper method (inverse-FFT resynthesis) cannot
express.

The scene: 448 singers in four parts, every harmonic up to 20 kHz a copy of
the partial, formants of the vowel "a", a four-chord progression. That is
**48,048 partials**, so 48,048 small signal processors running at once.

- The best CPU code Faust produces for this partial (LLVM) costs 7.4 ns per
  partial and per sample on one core. With **all ten cores** busy, the machine
  computes at most about **22,600 partials** in real time, without even
  mixing them.
- The GPU computes the 48,048 partials, and mixes them, in **3.6 ms per
  block, 68 % of the budget**. The choir needs **2.1 times the whole CPU**.
- It plays in real time for 20 s without a dropout, and so do 576 singers
  (61,776 partials, 84 % of the budget).

On this machine, the GPU is the only way to play this choir. In throughput,
for this voice, the GPU is worth about **thirty CPU cores**.

The choir exists in two versions. The **dry** version is the one measured
above: four chords in close position, 5 s each. The version **in a hall**
adds a reverb that runs on the CPU (next section) and a richer score:

- the four registers are spread wider, from the basses' C2 to the sopranos'
  G5, and each part has its own vibrato (slower and narrower low, faster and
  wider high), its own breathiness, and its own place on the stage
  (sopranos on the left, then altos, tenors, basses on the right);
- after the first four chords (D minor, B flat, F, C) come four more that
  explore the harmony further: G minor 7, E flat (the Neapolitan sixth of D
  minor), A7 (the dominant, with its leading tone C sharp) and D major (a
  Picardy third), before the loop returns to D minor;
- each chord is followed by 3 s of rest, so that the hall's tail is heard
  alone.

Its harmonics stop at 16 kHz instead of 20 kHz, so that the deeper basses do
not raise the count: 448 singers make 46,592 partials, computed in about
3.5 ms per block.

### 6.4 Finding the modes of a sound: learning on the GPU

The previous sections play modes whose settings are already known. The
inverse problem is more common: one has a recording (a bell, a gong, a
struck plate) and looks for the modes that reproduce it, that is the
frequency, decay and gain of each. A real object has hundreds or thousands
of them. The work is done offline, on long blocks, with no latency
constraint: the ground where the GPU is most at ease.

**Each mode is a copy that knows how to correct its error.** The voice
(`tests/corpus/gpu_modal_fad_voice.dsp`) is a resonator struck by an
impulse, with three controls: `freq`, `t60` and `gain`. Thanks to `fad`, the
automatic differentiation of `faust-rs`, it outputs not only its sound but
also how that sound would change if each of its three settings were moved:

```faust
process = fad(gain * resonator(freq, t60), (freq, t60, gain));
```

The host adds the thousands of modes and compares the result with the
target. The gap, sample by sample, is called the *error*. Each mode
multiplies that error by its three derivatives and sums the product over the
signal: it then knows which way to move its settings. Such a pass over the
whole signal is called an *epoch*, and epochs repeat until the error stops
falling.

```text
target ─────────────────────┐
                            ▼
mode 1 ─┐                ┌───────┐
mode 2 ─┼── sum ────────▶│  gap  │── error e ──┐
  ...   │                └───────┘             │
mode N ─┘                                      ▼
   ▲          each mode: sum of e × its 3 derivatives
   └────────── new settings (one GPU thread per mode)
```

Two choices make this fast on the GPU:

- **One pass per epoch.** The error at one instant depends only on the mix
  at that instant. So the work can move block by block: every mode computes
  a block, the mix gives that block's error, and each mode confronts its
  derivatives with it before moving to the next block. No derivative is kept
  beyond one block.
- **Each mode solves its own small problem.** The usual method (Adam) takes
  small steps in the right direction. Here each mode also works out how its
  three derivatives resemble one another, and solves a system of three
  equations to jump straight to its best setting (Gauss-Newton). It
  converges in about fifteen epochs where Adam needs about 150. When two
  neighbouring modes correct the same error at once, both can overshoot; a
  safeguard then undoes the step and shortens it.

**The cost of one epoch**, for one second of sound. On the CPU, one copy per
mode is limited by the chain of its recursion. Four modes in one Faust
program (`tests/corpus/gpu_modal_fad_group.dsp`), compiled by LLVM, go twice
as fast: that is the fair baseline.

| modes | GPU | CPU, 10 threads | GPU gain |
|---|---|---|---|
| 1,024 | 14.8 ms | 42 to 49 ms | ×2.8 |
| 4,096 | 68 ms | 162 to 172 ms | ×2.4 |
| 16,384 | 292 ms | 622 to 680 ms | ×2.1 |

Before the compiler kept the state local (§5), the same learning took 75 ms
at 1,024 modes on the GPU and lost to the CPU.

**What the learning recovers.** To check the result mode by mode, the target
is first synthetic: a hidden bank of modes, rendered by the same voice. The
start is σ bandwidths away from the truth. A mode's bandwidth is the width
of its spectral line, narrower the longer the mode rings. A mode counts as
recovered when its frequency is right to within 1 % of its bandwidth, and
its decay and gain to within 1 %.

| start | modes recovered out of 1,024, after 300 epochs (Gauss-Newton) |
|---|---|
| σ = 0.25 | 1,021 |
| σ = 0.5 | 1,001 |
| σ = 1 | 914 |
| σ = 2 | 268 |

At σ = 0.5 the error is close to its minimum by the fifteenth epoch, 0.2 s
of GPU time.

Two limits, more important than the speed:

- **The learning refines; it does not search.** Beyond about one bandwidth
  away, a mode no longer "sees" the line it should reach. A real tool would
  start with an analysis of the sound (finding the peaks of the spectrum)
  that places each mode close enough, then let the learning finish the job.
- **Too many modes for the signal, and the answer is no longer unique.** At
  4,096 modes between 100 Hz and 16 kHz, the modes are closer to each other
  than their bandwidths. Over one second of sound, several sets of modes
  then explain the same signal: the error falls, but the settings do not
  reach those of the hidden bank. A longer signal, or a constraint on the
  number of modes, would be needed.

**On a real bell.** For a recording, each mode also gets its own phase (a
second amplitude, that of the cosine,
`tests/corpus/gpu_modal_fad_voice_phase.dsp`). The starting point comes from
the sound itself: the peaks of the spectrum, each one's decay, and the
amplitudes of all the modes computed together (`tools/modal_init.py`). Two
bells from Wikimedia Commons were tried:

- **a small bell held by hand** does not obey the model: its partials glide
  by several hertz and their level rises again afterwards. The learning
  gains almost nothing there;
- **the hour strike of St. Georgen** (St. Gallen), a fixed bell struck by its
  hammer, suits it. Over 3.8 s of the last strike, 300 modes go from 6.3 dB
  (the ratio of the target to what is left to explain, at the start) to
  17.9 dB after 1,500 epochs, 98 % of the energy explained. What is left:
  the end of the decay, a beating doublet of the hum, and the treble above
  2 kHz, where the start placed no mode.

More modes do not help: at 600 the result gets worse.

**Is the GPU faster here?** Hardly. The comparison is the same learning on
the CPU (same target, same starting modes, same update), four modes per
Faust program compiled by LLVM, on ten threads:

| modes | GPU, one epoch | CPU, 10 threads | GPU gain |
|---|---|---|---|
| 100 | 38 ms | 22 to 23 ms | ×0.6: the CPU wins |
| 200 | 37 ms | 41 ms | ×1.1 |
| 300 | 36 to 38 ms | 59 to 65 ms | ×1.6 to ×1.8 |
| 600 | 45 ms | 115 ms | ×2.6 |

The GPU's time hardly moves between 100 and 300 modes. It does not count
the modes; it pays the time one of its threads takes to run through the
182,000 samples of its recursion, and a few hundred copies are too few to
hide that wait. The CPU pays in proportion to the modes, and the two meet
at about 200. A real bell, with 100 to 300 useful modes, sits below the
crossover of §6.1: here the method serves, not the speed. The command is in
§9.2.

## 7. CPU and GPU in the same program

Nothing obliges a program to run entirely on the GPU, or entirely on the CPU.
Mojo compiles **one** program whose functions run on either: some are
launched on the GPU (the *kernels*), the others are ordinary CPU code. Faust
code can therefore be placed on both sides of the same program, each piece
where it is best served.

The choir in a hall shows it. Its reverb (`re.zita_rev1_stereo`, in
`tests/corpus/gpu_choir_reverb.dsp`) is **one** program applied to the stereo
mix: exactly what the GPU does badly (§2). So the work is split, block after
block:

```text
   CPU                         GPU                               CPU
+-----------------+    +---------------------------+    +---------------------+
| score: chord    |    | 48,048 partials           |    | reverb              |
| changes, the    |--->| (Faust, batch form)       |    | (Faust, ordinary    |
| controls of the |    |            |              |    |  form)              |
| partials        |    |            v              |    |         |           |
+-----------------+    | mix: each singer,         |--->|         v           |
                       | then stereo               |    | sound card          |
                       +---------------------------+    +---------------------+
                                        2 x 256 samples
```

- The **CPU** runs the score: at a chord change, it computes the new
  frequency and gain of every partial and sends them to the GPU.
- The **GPU** computes the 48,048 partials and sums them into two channels:
  3.6 ms per block.
- Only those two channels, 256 samples each, come back. The **CPU** passes
  them through the reverb and hands them to the sound card: about **50 µs per
  block**, 1 % of the budget. The same reverb on the GPU would take 1 to
  2.6 ms.

Both Faust programs are compiled into the same Mojo program, in two forms:
the partial in the batch form (one structure for all the copies, its memory
laid out for the GPU), the reverb in the ordinary form of one instance. The
CPU version of the partial is in the same program too: it is the reference
the GPU is checked against.

The rule of thumb that follows: **put on the GPU what is many, keep on the
CPU what is one**. A master reverb, an equalizer or a limiter on the mix, a
sequencer, MIDI input, the user interface: all of these stay on the CPU,
next to thousands of voices computed on the GPU.

The two sides currently work one after the other within each block (the CPU
waits for the GPU's mix, then runs the reverb). They could overlap, the CPU
treating one block while the GPU computes the next; that was not needed here.

## 8. Latency: the smallest audio buffer

An audio program does not compute sample by sample for the sound card: it
computes **blocks** (buffers) of N samples, and the sound card plays one block
while the next is computed. A block of N samples lasts N / 48,000 s: that is
its **budget**. If a block takes longer than its budget to compute, the card
runs out of samples and the sound **crackles** (an underrun, or dropout).
Small blocks give a small latency, the delay between a gesture and the sound,
which matters for an instrument played live.

On a CPU, the cost of a block is almost entirely the computation of its
samples: calling the program's `compute` costs microseconds. Audio software
commonly runs with blocks of 32 to 64 samples (0.7 to 1.3 ms), and the
latency is set by the sound card's driver, a few milliseconds.

On a GPU, every block pays a **fixed cost** before the first sample: the CPU
prepares the work, launches it on the GPU, waits for the GPU to finish, and
copies the result back. That cost does not shrink with the block, and the
smaller the block, the larger its share of the budget.

### 8.1 What was measured

The fixed cost alone, measured with a choir of 4 singers (429 partials,
almost no work), blocks computed back to back: **0.34 ms** per block at 32 or
64 samples, 0.41 ms at 128, 0.55 ms at 256.

In real time it is worse. Between two blocks the GPU waits for the sound
card, goes idle, and lowers its clock; each block then pays the wake-up and
the submission again. At 64 samples, a block costs about **1 ms** whatever the
size of the choir, three times the back-to-back figure, and some blocks take
3 to 5 ms (the *jitter*). The choir in real time (PortAudio, 8 s, muted):

| block | budget | result | median block | worst block | latency granted |
|---|---|---|---|---|---|
| 256 samples | 5.33 ms | 448 singers (48,048 partials): no dropout | 3.6 ms | 10 to 17 ms | 43 ms |
| 128 samples | 2.67 ms | 448 singers: no dropout | 1.98 ms | 4.9 ms | 30 ms |
| 64 samples | 1.33 ms | dropouts, even with 32 singers (3,432 partials) | 1.0 ms | 3.0 to 3.4 ms | 23 ms |
| 32 samples | 0.67 ms | dropouts, even with 8 singers (858 partials) | 0.57 ms | 2.2 to 3.2 ms | 20 ms |

**On this machine, the smallest block that plays without crackling is 128
samples (2.7 ms), with an output latency of about 30 ms.** At 64 samples the
median block fits in its budget, but the fixed cost leaves no room for the
jitter, and the blocks over budget accumulate faster than the buffer can
absorb them. A smaller choir does not help: below a few thousand partials the
block costs the fixed cost, not the work.

This is the main practical constraint of the GPU compared with the CPU. A
CPU plays a light program at 32 samples with a few milliseconds of latency; a
GPU, even with almost nothing to compute, needs 128 samples and, with the
player used here, about 30 ms.

### 8.2 How the player drives PortAudio

The harness talks to the sound card through PortAudio's **blocking** API
(`Pa_WriteStream`), from the same thread that drives the GPU. Per run:

1. `Pa_OpenStream` opens the default output: 2 channels, 32-bit floats,
   48 kHz, N frames per buffer, a *suggested* latency of two blocks, and no
   callback (which selects the blocking mode). `Pa_GetStreamInfo` returns the
   latency actually granted.
2. Before the stream starts, 20 blocks are computed and thrown away, so that
   the GPU reaches its clock and the first blocks are not slow.
3. Then, for every block:
   - the CPU applies the score (at a chord change it uploads the new
     controls of every partial);
   - the CPU launches the GPU work (the partials, then the two mix stages)
     and a copy of the 2 x N mixed samples back, then **waits** for the GPU;
   - with the hall, the CPU runs the reverb on those samples;
   - `Pa_WriteStream` hands the N frames to PortAudio. PortAudio keeps them
     in its buffer; if the buffer is full, the call **waits** until the card
     has played enough. This wait is what paces the loop at the speed of the
     sound card.
   - If the card ran dry since the previous write, `Pa_WriteStream` returns
     `paOutputUnderflowed`: the harness counts a dropout.

```text
every block of N samples:

  CPU (harness thread)        GPU                PortAudio buffer    sound card
      |                        |                       |                 |
      |-- controls (at a ----->|                       |                 |
      |   chord change),       |                       |                 |
      |   launch partials      |                       |                 |
      |   and mix              |                       |                 |
      |<-- 2 x N mixed --------|                       |                 |
      |    samples (the CPU    |                       |                 |
      |    waits for them)     |                       |                 |
      |                        |                       |                 |
      |-- reverb (version in the hall)                 |                 |
      |                                                |                 |
      |-- Pa_WriteStream (waits while the buffer ----->|                 |
      |   is full)                                     |-- plays ------->|
      |                                                |   continuously  |
```

The latency granted (43 ms at 256 samples, 30 ms at 128) is that buffer: it
is what lets a slow block (a chord change takes 10 to 17 ms, three blocks of
256 samples) pass without a dropout, as long as the following blocks are fast
enough to refill it. With the blocking API, PortAudio adds its own buffering
on top of the driver's, so the latency is larger than the two blocks asked
for.

### 8.3 What could lower it

- **A player driven by the audio callback** (PortAudio's callback API, or
  Core Audio directly) would bring the buffering down towards the driver's
  minimum. But the callback runs on a real-time thread, which must never
  wait for the GPU: the wait at the end of every block is a blocking system
  call, and its duration is not guaranteed. The sound design is a separate
  computing thread that keeps a few blocks ahead, and a lock-free queue
  between it and the callback. The latency is then the depth of that queue,
  which the jitter dictates: with 3 to 5 ms of jitter at 128 samples, two or
  three blocks, about 6 to 8 ms of queue plus the driver's own buffer. Not
  written or measured.
- **Overlapping blocks**: launching block k + 1 on the GPU while block k
  goes through the reverb and to PortAudio hides part of the fixed cost.
- **Keeping the GPU awake** between blocks would remove the real-time
  penalty (1 ms instead of 0.34 ms at 64 samples), at the price of power.

None of this removes the fixed cost of a round trip to the GPU, so the GPU
will stay behind the CPU for the smallest buffers. Its domain is the sound
that can afford a few tens of milliseconds: installations, sound masses,
spatial scenes, offline rendering, training; not the monitoring of a played
instrument.

## 9. Trying it

The GPU path is an experimental test lane, not yet a product. It needs a Mojo
toolchain (Mojo 1.3 and the `modular` package) and, to listen, PortAudio. The
commands are in [`tests/impulse-tests/README.md`](../tests/impulse-tests/README.md).

### 9.1 Installing the toolchain

The measurements used a Mojo 1.3 nightly (`1.3.0.dev2026110705`, with
`modular` `26.8.0.dev2026110705`) and PortAudio from conda-forge, installed
with [pixi](https://pixi.sh) in an environment of their own. A file
`pixi.toml` in an empty directory:

```toml
[workspace]
channels = ["https://conda.modular.com/max-nightly", "conda-forge"]
name = "mojo-gpu"
platforms = ["osx-arm64"]

[dependencies]
mojo = "*"
modular = "*"
portaudio = ">=19.7.0,<20"
```

Then `pixi install` in that directory. The two values the commands below
need are the Mojo command and the directory of the PortAudio library:

```bash
MOJO="pixi run --manifest-path <env>/pixi.toml -q mojo"
```

```bash
PORTAUDIO_LIB=<env>/.pixi/envs/default/lib
```

Mojo nightlies change often; pinning `modular` to the version above
reproduces the measured toolchain. The first build of a lane takes about a
minute (the Mojo compilation of the harness and the generated programs).

### 9.2 Playing and rendering

The choir, dry:

```bash
make -C tests/impulse-tests mojo-gpu-choir MOJO=<mojo command> PORTAUDIO_LIB=<dir> POLY_AUDIBLE=audible
```

The choir in the hall, the reverb on the CPU:

```bash
make -C tests/impulse-tests mojo-gpu-choir-reverb MOJO=<mojo command> PORTAUDIO_LIB=<dir> POLY_AUDIBLE=audible
```

Once built, the program can be played directly, and the hall adjusted:
`dry` and `wet` are the levels of the direct and reverberated sound (0.4 and
2 by default, the hall about 3 dB above the choir), `t60` the decay time in
seconds (5 by default), `gap` the rest after each chord (3 s by default):

```bash
tests/impulse-tests/build/mojo-gpu-choir/choir play 448 30 audible
```

```bash
tests/impulse-tests/build/mojo-gpu-choir/choir play 448 64 audible reverb wet=3 t60=7 gap=4
```

A full cycle of the version in the hall (eight chords and their rests) lasts
64 s. Measure or listen to one choir at a time: two choirs started together
share the GPU, each takes about twice as long per block, and both break up.

Both versions can also be rendered offline to WAV files, to listen without
the toolchain: `make -C tests/impulse-tests mojo-gpu-choir-render` writes
`build/mojo-gpu-choir/choir-dry.wav` (20 s) and `choir-reverb.wav` (69 s).
The GPU renders them faster than real time.

The carillon:

```bash
make -C tests/impulse-tests mojo-gpu-bells MOJO=<mojo command> PORTAUDIO_LIB=<dir> BELLS=37 BELLS_SCORE=chime POLY_AUDIBLE=audible
```

Each lane checks the GPU against the CPU, times both, then plays.

The learning of the modes (§6.4), without PortAudio:

```bash
make -C tests/impulse-tests mojo-gpu-modal-fad MOJO=<mojo command> MODAL=4096
```

It checks the derivatives and the GPU code, times one epoch on the GPU (with
the state kept local, then without) and on the CPU, then runs a learning of
`MODAL_EPOCHS` epochs from a start `MODAL_SIGMA` bandwidths away, with
`MODAL_UPDATE` (`gn` or `adam`).

The same learning on a recording one provides (sox and numpy are needed
too):

```bash
make -C tests/impulse-tests mojo-gpu-modal-fad-bell MOJO=<mojo command> BELL_WAV=<file> BELL_START=<s> BELL_SECONDS=4
```

It writes the modes found, and the target, the model and the residual as WAV
files at the same gain, to `tests/impulse-tests/build/mojo-gpu-modal-fad-bell/`.

The Faust side is ordinary Faust. The partial of the choir starts like this:

```faust
freq = hslider("freq", 440, 1, 20000, 0.001);
gain = hslider("gain", 0.01, 0, 1, 0.0001);
vrate = hslider("vrate", 5.5, 0, 20, 0.001);
...
process = gaing * swell * ((1 - noisiness) * partial + noisiness * nscale * bandpass(noise));
```

Nothing in it says "48,048": the host program creates that many copies and
sets each one's controls.

### 9.3 Writing your own GPU program

For a developer who wants to run another Faust program on the GPU, the
pieces are these.

- **The batch form of the program.** `faust-rs -lang mojo` writes the
  ordinary form, one structure per instance. The batch form, one structure
  for N instances, has no command-line option yet; the compiler's example
  writes it:

  ```bash
  cargo run --release -p compiler --example mojo_batch -- voice.dsp --cn voicedsp -o voice.mojo
  ```

  Its state is kept local during the loop (§5); `--no-registers` writes the
  earlier form, for comparison.

- **What the batch structure offers.** It is built over one buffer of state,
  allocated by the host on the GPU: `state_bytes(voices)` gives its size, and
  `voicedsp(voices, state)` builds the structure over it. `class_init(rate)`
  runs once for all instances; every other method takes the instance first:
  `instance_init(voice, rate)`, `set_control(voice, index, value)`,
  `get_control(voice, index)`, `compute(voice, count, inputs, outputs)`.
  The index of a control is its position in `build_user_interface`, which
  a host reads once to map labels to indices.
- **What the host writes.** The GPU *kernels*, each running one instance per
  GPU thread: one calls `instance_init`, one `set_control` with a value per
  instance, one `compute`. The tables of input and output pointers of every
  instance (a mode reads its bell's strike there). The mix, best in two
  stages: one GPU thread per sample summing every mode made the carillon
  slower than the CPU; one per bell, then one per sample over the bells,
  made it faster. And the loop of §8.2 around PortAudio.
- **Three rules learned on Metal**, each a silent failure when broken: every
  buffer a kernel reaches must be one of its arguments; the pointer tables
  must live in GPU memory; kernel arguments must be fixed-width types.
- **Models to start from**, in `tests/impulse-tests/archs/`:
  `choirmojogpubatch.mojo` (one program, thousands of instances with their
  own controls, a CPU reverb, PortAudio, offline render),
  `bellsmojogpubatch.mojo` (two programs chained on the GPU, the strike
  feeding its modes), `polymojogpubatch.mojo` (polyphony driven by
  scripted notes) and `modalfadmojogpubatch.mojo` (thousands of instances
  with their derivatives, the sums and every instance's update on the GPU).

## 10. The current limits

The approach is real, measured and validated, but narrow. Its limits are as
important as its gains.

**It only suits one shape of problem.** Thousands of independent copies of
the same program. One heavy program (a large reverb, a big physical model)
runs slower on the GPU, and ordinary polyphony is below the crossover. A
large mesh (a membrane, a plate simulated point by point), an FFT convolution
or a mixing matrix are parallel *inside* one program: Faust writes them as
one program, and this approach does not help them.

**Latency and block size** (§8). Each block pays a fixed cost (0.34 ms back
to back, about 1 ms in real time at 64 samples) before any sound is
computed, plus several milliseconds of jitter. The smallest block that played
without crackling is 128 samples, with about 30 ms of output latency through
PortAudio's blocking API; a CPU plays light programs at 32 to 64 samples with
a few milliseconds. That suits an installation, a sound-mass piece or offline
rendering, not live monitoring of an instrument.

**Precision.** Consumer GPUs compute in 32-bit floating point (`f32`), not in
64-bit. High-Q filters, very low frequencies, feedback networks sensitive to
rounding drift away from the CPU result. Building the choir met a classic
trap: a glide of frequency computed in `f32` stops short of its target (by
about 0.4 cent), at a slightly different point on GPU and CPU, and the phases
then drift apart. The voice now glides only its gain.

**Not every Faust program can go to the GPU.** Of the 133 programs of the
impulse-response test suite, 131 run on it. The two others need what the
Apple GPU cannot give: `sound` reads a soundfile, which the Mojo backend
refuses (as the C++ one does), and `math` computes an integer power in
64-bit, which the GPU does not have. `-double` is impossible there for the
same reason, and the Mojo backend refuses the `-ec`/`-os` modes. Of the 131,
about 120 give the CPU's result within 1e-5. Eight drift away through their
feedback, from 0.08 % (`thru_zero_flanger`) to 45 % (`phaser_flanger`) of
their peak over 15,000 samples: the difference starts at the last digit (the
two compilers generate different floating-point code, and the GPU flushes
very small numbers to zero), and the feedback amplifies it.

**Everyone must do the same thing.** The units of a group execute the same
instruction. If voices take different branches (a different rate per voice,
iterations that differ), they wait for each other: the result stays right,
the speed drops.

**Memory.** Each copy has its own memory: 4,096 copies of `zita_rev1` need
2 GB.

**The host program is written by hand.** Faust describes one copy. Feeding
the copies, linking them (the bell strike feeds its 50 modes), summing them
into a stereo mix, and driving the sound card are written today in Mojo test
harnesses. The "batch" output (one structure for N voices) has no
command-line option yet; it is reached through the compiler's Rust API
(§9.3).

**Mojo is not faster than C++ on the CPU.** On the choir's partial, the
generated Mojo code ran five times slower on the CPU than the same program
compiled by LLVM from C++. The interest of Mojo here is the GPU; the CPU
comparisons of this document use the LLVM code.

**A young toolchain.** Mojo changes from one version to the next, and this
work met three failures in its GPU path, two of them silent. The lanes are
outside the continuous integration (which has no Mojo), and the C++ Mojo
backend whose contract `faust-rs` follows is not merged yet, so that contract
may still move.

**The comparison depends on the CPU side.** The figures compare the GPU with
the best CPU code Faust produces today. A CPU version vectorized by hand
across voices (four or eight voices per instruction) was not measured, and it
could narrow the gap, possibly close it for small programs. The carillon
showed how much a fairer CPU baseline can change a conclusion.

**A GPU shared with other work.** The GPU is not reserved for the sound.
During these measurements, two choirs started at the same time shared it:
each took about twice as long per block (5.5 to 6.1 ms instead of 3.6) and
both crackled. Anything else that uses the GPU (another audio program, a
video, an application's display) takes its share of the same budget, and
an audio program has no way to reserve the GPU for itself.

**One machine.** Everything was measured on an Apple M5 with its integrated
GPU. Not measured: power and heat, a GPU loaded by a heavy display (inside a
DAW, under a 3D interface), discrete GPUs where data crosses a bus.

## 11. Takeaways

| question | GPU worth trying | stay on the CPU |
|---|---|---|
| How many independent copies? | thousands or more | one, or a few hundred |
| Where is the parallelism? | across copies | inside one program |
| Work per copy | real work per sample | a few operations (group them on the CPU) |
| Latency | blocks of 128 samples or more, 30 to 45 ms acceptable, or offline | blocks of 32 to 64 samples, live monitoring |
| Precision | `f32` is enough | needs `f64`, rounding-sensitive feedback |

- `faust-rs -lang mojo` writes Mojo code for any Faust program, with the same
  results as C++ on the CPU.
- Thousands of copies of a Faust program can run on the GPU, unchanged; the
  compiler lays out their memory for it.
- In that regime the GPU brings from ×2 to ×5 over the CPU threads for
  programs of real weight, and it plays at least one program, a choir of
  48,000 rich partials, that the whole CPU cannot.
- For offline learning, such as finding thousands of modes with `fad`, the
  GPU brings ×2 to ×3 over the ten CPU cores. It refines a good starting
  point; it does not replace the analysis that provides it.
- CPU and GPU code mix in the same Mojo program: the thousands of copies on
  the GPU, the single programs (a reverb on the mix, a sequencer) on the CPU.
- For everything else (one program, ordinary polyphony, small blocks, `f64`)
  the CPU remains the right place.

## See Also

- [`vector-scheduling-synthesis-en.md`](vector-scheduling-synthesis-en.md):
  the other direction of parallelism, inside the processor (SIMD), including
  instances advancing in lockstep.
- [`fad-rad-synthesis-en.md`](fad-rad-synthesis-en.md): automatic
  differentiation, which meets the GPU in offline training (§6.4).
- [`user-cli-guide-en.md`](user-cli-guide-en.md): the `-lang mojo` option.
