# Running many voices of a faust-rs Mojo program on a GPU. Measurements and plan

Date: 2026-11-07

Status: **G1 and G2 landed 2026-11-07 on Mojo 1.3 (§4; outcomes §7, §8);
G2 calls for G3, designed in
`porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md` (G3.2, the
layout pass, G3.3, its checker, G3.4, the CPU differential, and G3.5, the Mojo
batch mode with its lanes, landed: large-state programs ×20 to ×31 faster at
4,096 voices, now ahead of the CPU threads); G4 not started.** §2 records a hand-written probe run
on the day; no file of the repository changed for it.

## Scope

`porting/mojo-gpu-pr-1293-analysis-and-batch-axis-2026-09-18-en.md` (§3)
argued that a GPU should take its parallelism from many *instances* of one
program (voices, channels, the copies of a training batch), not from the
loop graph of one instance as the `-gpu` path of grame-cncm/faust#1293
does. §3.3 there named a form that needs no compiler change: N instance
structures side by side on the device, each GPU thread running the
ordinary scalar `compute` of its own instance. Since then, faust-rs has a
Mojo backend (`porting/mojo-backend-port-analysis-and-plan-2026-11-06-en.md`),
so that form can be tried on this machine. This document measures both
approaches and plans the batch approach.

Machine of record: Apple M5 (4 performance and 6 efficiency cores, 32 GB),
Metal 4, Mojo `1.1.0.dev2026090705` with the `modular` package of the
pull request's `pixi.lock`. The device computes in `f32`.

## 1. The pull request's `-gpu` path, measured

The pull request's compiler (`05ac86fb1`), `-lang mojo -gpu -single`, with an
offline impulse harness written for the test (the calls of
`architecture/mojo/portaudio-terminal-gpu.mojo`, PortAudio replaced by the
impulse loop of `impulse.mojo`):

- 6 of 8 programs run and match the C++ references over the first 15,000
  frames (`APF`, `freeverb`, `karplus32`, `noise`, `stereoecho`,
  `reverb_tester`). `lowboost` and `zita_rev1` fail to build: Mojo 1.1 has
  no `tan` for the Apple GPU (`failed to run the pass manager for offload
  functions`). Programs with tables (`osc`) are refused by the pull
  request's compiler.
- Time per 64-frame block (1.45 ms of audio at 44.1 kHz):

| program | GPU | CPU, same compiler, scalar | ratio |
|---|---|---|---|
| APF | 629 µs | 0.24 µs | ×2,600 slower |
| freeverb | 1,225 µs | 2.2 µs | ×560 slower |
| karplus32 | 926 µs | 3.5 µs | ×270 slower |
| reverb_tester | 972 µs | 0.54 µs | ×1,800 slower |

The checksums of GPU and CPU agree. `APF` launches its kernels with
`grid_dim=1, block_dim=1`: one GPU thread, a few kernel launches and a
synchronisation per block. This confirms the September reading (§1.3
there): the path is correct and, by its design, not usable. It is not
ported.

## 2. The batch approach, probed by hand

### 2.1 Method

faust-rs `-lang mojo -single -cn mydsp`, output unchanged, concatenated with
the pull request's prelude and a hand-written harness:

- the host allocates N `mydsp` structures, runs `init` on each, and copies
  them to one device buffer;
- the kernel takes the voice index from `global_idx.x`, points at its
  structure, builds its channel pointers, and calls `compute` for a block
  of 256 frames;
- every block: input copy, one launch (`block_dim=64`), output copy,
  `synchronize`, timed together;
- the last voice is also run on the CPU, same code, and compared sample by
  sample.

### 2.2 Results

`APF` (32 bytes of state per voice), impulse on every voice:

| voices | GPU per block | the same voices on one CPU core | ratio | share of the real-time budget (5.8 ms) |
|---|---|---|---|---|
| 64 | 381 µs | 75 µs | ×0.2 | 7 % |
| 256 | 418 µs | 320 µs | ×0.8 | 7 % |
| 1,024 | 336 µs | 1.2 ms | ×3.5 | 6 % |
| 4,096 | 651 µs | 5.4 ms | ×8.3 | 11 % |
| 16,384 | 1.7 ms | 25 ms | ×15 | 29 % |
| 65,536 | 5.8 ms | 133 ms | ×23 | 101 % |

GPU and CPU agree to 9.5e-7 on every block. The ratio is against **one**
CPU core; against the four performance cores, divide it by about four. The
fixed cost of a block (transfers, launch, synchronisation) is about
0.35 ms, which is what bounds the block size from below (§3.5 of the
September analysis: 256 frames at 44.1 kHz pass, 32 would not).

### 2.3 Findings

- **faust-rs scalar Mojo runs on the device unchanged.** `compute`, with its
  `ref dsp = self` receiver alias, is called from a kernel as it is. Nothing
  in the generated text had to be edited.
- **Two programs with a large state write nothing.** `freeverb` (149,784
  bytes of state per voice) and `karplus32` (481,724 bytes) build, launch
  and return without error, and their device output stays at zero; the
  CPU run of the same voice is not zero. Their timings therefore measure
  nothing and are not reported. *Settled in G1 (§7): not the size; the
  channel pointers were in the thread's private memory.*
- **Failures are silent.** A kernel that builds its channel pointers in a
  local `Array` of pointers also writes nothing, without an error; the
  same pointers in a two-field struct work. The harness must check that
  every voice wrote something, not only that the launch returned.
- **Kernel arguments must be fixed-width**: an `Int` is refused
  (`Int and UInt do not conform to DevicePassable`), `Int32` passes.
- **No `tan` on the Apple GPU** in Mojo 1.1; other math functions may be
  missing too. Programs using one must be refused or routed through a
  helper.
- **Tables are fields.** The faust-rs Mojo backend flattens table
  generators into struct fields filled by `class_init`, so the batch form
  copies them with each voice: no refusal as in the pull request, at a
  memory cost of one table per voice.

## 3. The plan

Each phase has a deliverable and a pass criterion fixed before it starts
(AGENTS.md §5). The work is opt-in, outside CI, like the Mojo impulse
lane: CI has no GPU and no Mojo toolchain.

### G1 — Correctness on the device, all programs

Deliverable: an opt-in lane `make mojo-gpu` in `tests/impulse-tests`, built
on `Make.mojo`: a device harness (`archs/impulsemojogpu.mojo`) generic over
the DSP type, N voices (default 64) with the same inputs, every voice
compared with the C++ reference, and a check that every voice wrote a
non-zero sample where the reference has one.

First task inside G1: find why `freeverb` and `karplus32` write nothing
(struct size, private-memory limits, the call convention of `compute`),
and fix it in the harness or, if it comes from the generated text, in
the emitter.

Pass: every program of the impulse corpus either passes on every voice
within the impulse tolerance (`-part`, `f32`), or is refused before launch
with a stated reason (a missing device math function, a state too large
for the device memory). No silent zero output.

### G2 — A measured cost model

Deliverable: a timing mode of the same harness: N swept, block size swept
(64, 128, 256, 512), per-block time split into transfer, launch and
compute; baseline = the same voices on one core *and* on all performance
cores (a thread per core); output checksum kept with every row; results
written to `tests/impulse-tests/build/` and summarised in this document.

Pass: the `APF` row of §2.2 reproduced within 20 %, and a statement, per
program, of the voice count from which the device beats the four
performance cores, or that it never does.

### G3 — State layout for large programs (only if G2 says so)

*G2 said so (§8). Design, with a synthetic measurement (×31 at 4,096
voices) and the decisions to take:
`porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`.*

If G2 shows that programs with long delay lines lose on the device because
threads read their own structures at a stride of one structure (no
coalescing), a structure-of-arrays layout: each state field becomes an
array indexed by voice, so that the threads of a SIMD group read
consecutive addresses. As a FIR-to-FIR pass, not in the emitter: it is a
layout change whose errors would be silent, so it comes with an
independent checker (every voice of the transformed program against the
scalar CPU program, the G1 gate) and rejecting mutations (swap two voices,
offset a stride).

Pass: on the programs G2 named, a stated speedup over the array-of-
structures form at 1,024 voices; if the measured gain is under ×2, the
pass is not landed and the result is recorded.

### G4 — A real-time polyphonic runner

Deliverable: an architecture file (not a compiler change) that plays N
voices of one program through PortAudio, voice controls per structure,
the mix summed on the device or on the host; latency stated (block size,
round trip).

Pass: a polyphonic instrument with at least 256 voices plays without
underruns at 256 frames, 48 kHz.

### Later, not planned here

- training batches for `faustprobe --train` (one voice per starting point
  or per example), which reuses G1 and G2 unchanged;
- other device targets (WGSL through `wgpu`, Metal Shading Language, CUDA):
  the September analysis §6 keeps that decision open; the batch form needs
  only a device-compatible scalar emitter, which the Mojo backend already
  is.

## 4. Decisions open before G1

- **Mojo version. Decided 2026-11-07: the latest 1.3 nightly**
  (`1.3.0.dev2026110705` on the day, with its device package), not the 1.1
  the pull request pins. 1.3 fixes the receiver hang and is where device
  math functions are being added; §2 was measured on 1.1 and is re-measured
  on 1.3 in G1.
- **Shared or per-voice tables.** Per-voice copies are free in compiler
  work and cost memory; sharing them needs the tables out of the instance
  structure (a layout change, as in G3).
- **Per-voice controls.** Each structure has its own zones, which suits
  polyphony; a training batch wants per-voice parameters filled by the
  host, which already works the same way.
- **`f64`.** The Apple GPU has none; programs that need `-double` are out of
  the device path, or run in `f32` with their tolerance stated.

## 5. Risks

- Silent device failures (§2.3): the per-voice check of G1 is the
  mitigation, and no timing is reported without it.
- The fixed cost per block (about 0.35 ms here) excludes small blocks;
  real-time use stays at 256 frames or more.
- Mojo moves release to release, and its device library more than its
  language; every result is tied to a version (§4).
- The ratio against one core flatters the device; G2 reports against all
  performance cores.

## 6. References

- `porting/mojo-gpu-pr-1293-analysis-and-batch-axis-2026-09-18-en.md` §1.3,
  §3, §6.
- `porting/mojo-backend-port-analysis-and-plan-2026-11-06-en.md`.
- grame-cncm/faust#1293 at `05ac86fb1`: `compiler/generator/mojo/mojo_instructions_gpu.cpp`,
  `architecture/mojo/gpu/device.mojo`, `architecture/mojo/portaudio-terminal-gpu.mojo`.

## 7. G1 outcome (2026-11-07)

Mojo `1.3.0.dev2026110705`, `modular` `26.8.0.dev2026110705`, Apple M5.

**What landed.** `make mojo-gpu` (`tests/impulse-tests/Makefile`,
`Make.mojo`), the device harness `archs/runimpulsemojogpu.mojo` concatenated
after the CPU runtime `archs/impulsemojo.mojo` and the faust-rs output, as
the CPU lane does. 64 voices per program, `-single`. Even voices get the
impulse protocol of the CPU lane, odd voices silence; buttons are held in
the first block for all. The harness fails the program unless:

- every even voice is bit-identical to voice 0 and every odd voice to
  voice 1 (a voice reading another voice's state breaks one of the two);
- voices 0 and 1 match the same `f32` program run on the CPU in the same
  process, impulse and silence, within 1e-4 × max(1, peak of the CPU
  output);
- voice 0 is not silent where its CPU run is not.

**The oracle is the same program on the CPU, not the C++ reference.** The
references are `f64`; the CPU Mojo lane run in `-single` (`f32`) passes
only 85 of 132 programs against them, and the GPU lane failed on the same
programs for the same reason. Device execution is what G1 checks, so the
GPU is compared with the host running the identical text; correctness of
the text against C++ stays with `make mojo` (`-double`, 132/132). This
replaces the pass criterion of G1 in §3 ("within the impulse tolerance,
`-part`, `f32`").

**Result: 131/131 programs pass** (133 in the corpus; `sound` uses a
soundfile, `math` computes an integer power through `f64` even in
`-single`, as C++ does, and the Apple GPU has no `f64`; both are
`KNOWN_FAIL_mojo-gpu`). About 120 programs give the same output as the
CPU or differ at the last ulp (relative 1e-5 or less; 1e-38 where the GPU
flushes denormals). The lane builds and runs in about 2.5 minutes at
`-j4`.

**Eight programs drift** (`MOJO_GPU_DRIFT` in `known.mk`): the GPU/CPU gap
starts at rounding level and grows with feedback, for instance
`cubic_distortion` crosses 1e-7 at frame 85, 1e-5 at 547, 1e-3 at 2,942;
three programs jump at frame 950 and `thru_zero_flanger` at frame 265,
where a delay index or a comparison flips on that small difference. The
largest gaps over the 15,000 frames, relative to the peak: `phaser_flanger`
0.45, `parametric_eq` 0.28, `spectral_tilt` 0.17, `cubic_distortion` 0.077,
`modulations` 0.020, `vcf_wah_pedals` 0.0083, `bells` 0.0019,
`thru_zero_flanger` 0.00083. They run with every check but the GPU/CPU
tolerance. The source of the first difference is not isolated: the device
math functions are not it (`tan`, `sqrt`, `atan` identical to the CPU over
(0, 1.5), `sin`, `cos`, `exp`, `log`, `pow` within 1.3e-6 relative), and
both sides contract multiply-adds; the code generation differs, and the
GPU flushes denormals.

**Findings.**

- *The silent failure of §2.3 is a pointer into private memory.* The
  probe built the channel tables (`ImmStreams`, `MutStreams`) in the
  thread's private memory. `compute` of a large program is not inlined; a
  pointer to private memory passed to it loses its writes on Metal, with no
  error: `freeverb`'s thread runs, `compute` returns, nothing reaches the
  output. With the tables in device memory, written by a set-up kernel,
  every program writes. The generated text did not change. Likely a Mojo
  defect (the address space of a private pointer lost across a call); not
  reported yet, a reproducer is to be reduced first.
- *Mojo 1.3 removed `UnsafePointer`*: the harness uses `Pointer` with
  `MutAnyOrigin`, as the pull request does. Kernel arguments must be
  fixed-width (`Int32`, `Int64`), not `Int`.
- *`tan` now builds for the Apple GPU*, which 1.1 refused (`lowboost`,
  `zita_rev1` pass).
- *Tables travel with each voice*, as §2.3 expected: `osc`, `table`,
  `table2` and the waveform programs pass.

**Not done in G1.** No timing (G2); the voice count is fixed at 64
(`MOJOGPUOPTIONS="-D NVOICES=<n>"` changes it).

## 8. G2 outcome (2026-11-07)

Same machine and toolchain as §7. `make mojo-gpu-bench`
(`tests/impulse-tests/Make.mojogpubench`, harness
`archs/benchmojogpu.mojo`) builds five programs once each and runs them over
64 to 65,536 voices and blocks of 64 to 512 frames, serially, writing
`build/mojo-gpu-bench/results.csv` (not versioned). Per configuration: the
full block (inputs in, launch, outputs out, synchronise), the launch alone,
the copies alone and an empty kernel, each the median of 100 blocks after
20 warm-up blocks; the same voices on 1 CPU thread and split over 4 and 10
threads with `parallelize` (CPU timings taken on at most 4,096 voices and
scaled above); voice 0 compared with a CPU instance fed the same blocks
(gap 9.5e-7 to 3.9e-5; 0 for `freeverb` and `karplus32`). Inputs
are a low-level sine, so no state decays into denormals. Voice counts whose
state exceeds 4 GB are skipped.

**Two findings change the reading of §2.2.**

- *Warm-up matters on the device.* Over 20 blocks the 1,024-voice `APF`
  block measured 360 µs, over 200 blocks 235 µs; the probe of §2.2 had no
  warm-up and took a mean. Its small-count figures were cold: 1,024 voices
  at 256 frames now take 215 µs (§2.2: 336 µs), 4,096 take 372 µs (651).
  The large counts reproduce within 10 % (16,384: 1,535 µs against 1,700;
  65,536: 6,371 µs against 5,847). The first pass criterion of G2 holds for
  those, and not for the small counts, where the measurement of §2.2 was
  the defect.
- *A fixed floor of about 215 µs per block*: an empty kernel and its
  synchronisation, independent of the voice count and the block size. The
  copies add up to 4.4 ms at 65,536 `APF` voices of 256 frames (134 MB per
  block, in and out).

**Where the device beats the CPU.** The CPU baseline is the faster of 4 and
10 threads (10 is rarely faster: the 6 extra cores are efficiency cores and
the slices are equal):

| program | state per voice | GPU faster than the CPU from | best ratio, GPU vs CPU threads (vs 1 core) |
|---|---|---|---|
| `APF` | 32 B | 1,024 voices at 256–512 frames, 4,096 at 64–128 | ×3.6 at 4,096 voices, 256 frames (×13.7) |
| `osc` | 256 KB (its table) | never (up to 4,096) | ×0.87 at 4,096, 256 frames (×1.9) |
| `freeverb` | 150 KB | never | ×0.56 at 1,024, 64 frames (×1.2) |
| `zita_rev1` | 480 KB | never | ×0.58 at 1,024, 64 frames (×1.7) |
| `karplus32` | 482 KB | never | ×0.47 at 256, 256 frames (×1.7) |

The second pass criterion of G2 is met: a crossover for `APF`, "never" for
the four others. Real-time capacity on the grid measured (powers of four
of the voice count) is the same on the device and on the CPU threads for
`APF` (16,384 voices), `osc` (4,096) and `karplus32` (256), and lower on
the device for `freeverb` and `zita_rev1` (256 against 1,024). For `APF`,
the device does not play more voices; it frees the CPU cores.

**The programs with large state get worse as voices are added.** `freeverb`
at 256 frames: 1.3 ms for 256 voices, 6.5 ms for 1,024 (×5 for ×4 voices),
102 ms for 4,096 (×16 for ×4). The launch alone carries it (the copies
take 0.7 ms at 4,096 voices), so the cost is the kernel reading its state: 150 KB per voice
is 600 MB for 4,096 voices, far beyond the device caches, read by threads
that each walk their own structure at a stride of one structure. That is
the condition §3 set for G3, so **G2 calls for G3** (structure-of-arrays
layout), for `freeverb`, `zita_rev1` and `karplus32` first. For `osc`, the
state is its 256 KB table, the same for every voice: sharing the tables
(§4) is the cheaper fix there, and G3 would not help a table read at a
data-dependent index.

