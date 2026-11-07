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

**No, for everything else**, in particular:

- **one instance of a program, however heavy.** A single reverb runs 30 to 100
  times slower on the GPU than on one CPU core;
- **ordinary polyphony.** Tens to a few hundred voices sit below the crossover,
  which is about 1,000 voices for small programs and about 4,000 for large ones;
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
  threads, and ×7 to ×18 that of one core.
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
  about 4,000 for large ones, the CPU threads win. Most musical uses of Faust
  sit far below that.
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
- **The CPU baseline is not the strongest possible.** The CPU numbers run one
  voice per call, without SIMD across voices. The same structure-of-arrays
  layout would let the CPU process 4 to 16 voices per instruction, which would
  narrow the gap, possibly to nothing for small programs. This has not been
  measured, and it is the most important caveat on the ratios above.
- **Not measured:** power and heat; a GPU shared with graphics (inside a DAW
  or with a visual front end); discrete GPUs, where copies cross a bus rather
  than unified memory.

## 5. Algorithm by algorithm

### Additive synthesis: a good fit at scale (measured)

Each partial is one instance of a small oscillator program with its own
frequency and gain, and the partials are summed on the device. Measured at
256 frames:

| partials | GPU | CPU threads | GPU vs CPU |
|---|---|---|---|
| 1,024 | 0.28 ms | 0.22 ms | ×0.8 |
| 4,096 | 0.33 ms | 0.79 ms | ×2.4 |
| 16,384 | 0.90 ms | 3.2 ms | ×3.5 |
| 65,536 | 3.1 ms | 12.7 ms | ×4.1 |

65,536 independently controlled partials fit in one 256-frame block, where the
CPU threads stop between 25,000 and 30,000.

Two qualifications:

- Writing the partials as `par(i, N, osc)` inside **one** Faust program gives
  one instance, so no GPU parallelism. The partials must be instances.
- For pure sinusoids with smooth control, inverse-FFT additive synthesis
  ("FFT⁻¹") is far cheaper on the CPU than any oscillator bank. The
  oscillator-bank GPU path wins when each partial is a real signal processor
  (its own envelope, noise, filter, nonlinearity), which the FFT method cannot
  express.

### Modal synthesis: a good fit (measured)

Each mode is a resonator instance (frequency, T60, gain). All modes are fed
the same excitation, which is broadcast as their input, and their outputs are
summed on the device:

| modes | GPU | CPU threads | GPU vs CPU |
|---|---|---|---|
| 1,024 | 0.22 ms | 0.30 ms | ×1.4 |
| 4,096 | 0.38 ms | 1.06 ms | ×2.8 |
| 16,384 | 1.6 ms | 4.3 ms | ×2.7 |
| 65,536 | 6.3 ms | 17.4 ms | ×2.7 |

This suits large modal models (bells, plates, rooms described by thousands of
modes) and scenes of many modal objects. The excitation input and the output
sum are the parts to keep on the device: a summing kernel did this in the
runner. In `f32`, high-Q, low-frequency modes are the precision risk, because
their poles sit very close to the unit circle.

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
  the first thing to measure.
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
- The additive and modal measurements of §5 used two self-contained voices
  (a wrapped-phase sine partial, and a two-pole resonator with T60 decay). They
  were run with the bench harnesses at 256 frames, and are not part of the
  repository.
