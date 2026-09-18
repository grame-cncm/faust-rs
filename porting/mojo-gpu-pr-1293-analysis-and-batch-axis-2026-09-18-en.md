# The Mojo and GPU pull request of the C++ compiler, and what a GPU would mean for faust-rs. Analysis

Date: 2026-09-18

Status: **analysis, nothing landed.** §5 ranks what is worth taking, in
order; §6 lists the decisions to make before any of it becomes a plan.
Revised the same day: the benchmark figures re-read from the plot (§1.1),
the precision remark corrected (§1.2, §5), the architectural batch axis
(§3.3), the real-time uses of the instance axis (§3.5), the two-dimensional
grid (§3.4), the nuances on FFT and scan evaluation of recursions (§4.2),
the shape of the autograd bridge (§4.4), and what GEMM is and why Faust
does not produce one (§4.5).

## Scope

Pull request 1293 of `grame-cncm/faust` ("Add benchmark framework and Mojo
backend support with explicit SIMD emission and experimental GPU
processing", by manuelfarzini, 98 commits, open on 2026-09-18) adds three
separate things to the C++ compiler: a benchmark framework under
`architecture/bench`, a Faust-to-Mojo backend under
`compiler/generator/mojo` with explicit SIMD emission for `-vec`, and an
experimental GPU path (`-gpu`, Mojo only). The question asked: is it worth
taking its ideas into faust-rs? Then two more: what "GPU" should mean here,
and how the classical differentiable-DSP (DDSP) systems occupy a GPU, and
whether faust-rs can get near them.

The sources read: the pull request description, the three READMEs
(`compiler/generator/mojo/README.md`, `architecture/mojo/README.md`,
`architecture/bench/README.md`), `mojo_instructions_gpu.cpp`,
`dsp/gpu.mojo`, and the benchmark plot `architecture/bench/report/plot/all.svg`
at the head of `manuelfarzini/faust@master-dev`. Nothing was run; the
numbers of §1 are the pull request's own, read from the text labels of the
SVG.

## 1. What the pull request does

### 1.1 The benchmark framework

Same Faust program, same conditions, C++ and Mojo, scalar and `-vec`; a
Pixi toolchain; results in a CSV, one row per case, replaced on rerun; SVG
plots; snapshots; a checksum of the output kept with every run, so that a
faster backend that computes something else is visible; and a separate
inspection path that emits LLVM IR and assembly without the benchmark
instrumentation. The configuration used keeps the DSP in `f64` with `f32`
architecture buffers on both sides.

The plot gives the throughput in frames per second for the 46 programs of
`architecture/bench/src` (the examples set), in four series. The rows
below are read from the SVG, whose labels are grouped by series:

| program | cpp scalar | cpp vec | mojo scalar | mojo vec |
|---|---|---|---|---|
| APF | 327 M | 262 M | 408 M | 414 M |
| freeverb | 99 M | 29 M | 52 M | 71 M |
| lowboost | 359 M | 277 M | 494 M | 349 M |
| highShelf | 359 M | 275 M | 446 M | 350 M |
| stereoecho | 2375 M | 1253 M | 1414 M | 1187 M |
| pitch_shifter | 183 M | 144 M | 107 M | 105 M |
| karplus32 | 38 M | 29 M | 39 M | 51 M |
| cubic_distortion | 6.0 M | 11.1 M | 6.1 M | 11.3 M |
| spectral_level | 3.4 M | 4.9 M | 3.4 M | 6.2 M |
| vcf_wah_pedals | 5.4 M | 8.0 M | 5.4 M | 9.4 M |
| virtual_analog_oscillators | 5.0 M | 7.0 M | 5.2 M | 8.4 M |

Three readings.

- **Mojo scalar against C++ scalar** goes from −47 % (freeverb) to +38 %
  (lowboost), with pitch_shifter at −42 % and stereoecho at −40 %; most
  programs are within ±25 %, and there is no program where Mojo wins by
  more than the spread one sees between two clang configurations. As a
  language, Mojo brings no throughput.
- **Where vectorization pays**, on the programs that cost most per sample
  (the ones near 5 to 10 M frames per second: cubic_distortion,
  spectral_level, vcf_wah_pedals, virtual_analog_oscillators), both C++
  `-vec` and Mojo `-vec` gain about ×1.7 to ×1.9, Mojo's explicit SIMD a
  little more than clang's autovectorizer. The explicit emission does what
  the autovectorizer does, no more.
- **Where it does not pay**, on recursive filters and delay networks, the
  C++ `-vec -vs 4` is *slower* than the C++ scalar, often by 20 to 70 %
  (freeverb 99 → 29 M). `-vs 4` is a small vector size, chosen because the
  Mojo backend requires `-vs` to equal the native `f32` width of NEON
  (§1.2); C++ Faust defaults to 32. On those programs Mojo's version loses
  less than clang's (APF 408 → 414 against 327 → 262), plausibly because its
  recursive sub-loops are unrolled with `comptime for` rather than run as a
  four-trip loop over vector buffers; the plot does not say.

The benchmark's most useful result is therefore negative, and it is a
result the framework made visible: the checksum and the four series side
by side are what make the comparison trustworthy.

### 1.2 The Mojo backend and its explicit SIMD

A text backend in the usual shape: `MojoInstVisitor` derived from
`TextInstVisitor`, `MojoCodeContainer` with scalar, vector and GPU
variants, a Mojo architecture (PortAudio, a terminal GUI in C11 behind an
FFI, bench, inspect and impulse helpers), integration in the impulse tests.

The SIMD emission exists because of a constraint: Mojo disables LLVM's
autovectorization passes, so the scalar emission of a `-vec` sub-loop is
never turned into vector instructions. The backend therefore lowers the
sub-loops of the `-vec` loop DAG to `SIMD[dtype, width]` operations itself:
recursive sub-loops fully unrolled over the vector size with `comptime for`,
independent sub-loops as SIMD loads, stores, casts and arithmetic; an
affine access analysis (`A[i]`, `A[i ± c]`, `A[c + i]`) that admits a
contiguous load, everything else (circular indices, calls, indirect
accesses) sent to the scalar fallback rather than to a wrong vector load,
since a contiguous load is not a gather; `f64` work split in two halves of
the `f32` lane count and joined on an `f32` store; bargraph updates
recognised as special loop shapes.

What makes it fragile, by the README's own list: the main loop is
recognised by the name `vindex`; the first element of the DAG is dropped
on the assumption that it initialises the index; every loop is assumed to
end with one main `StoreVarInst` that decides the strategy; bargraphs are
recognised by field names; `-vs` must equal `simd_width_of[f32]()` and this
is not checked; no tail loop is emitted, so the buffer size must be a
multiple of the vector size; the SIMD state is global and restored by hand
at the end of each loop; the validated configuration is exactly one,
`-double -vec -dfs -vs 4 -mcd 4` on Apple M1 and M4.

On precision, a correction to a first reading. "`f64` inside, `f32`
buffers" is not a peculiar choice: it is the ordinary deployment
configuration of Faust, `-double` with `FAUSTFLOAT` left as `float`. What
is peculiar is that the SIMD emission supports *only* that pair, and that
it needed dedicated C++ `vec4` references with the same pair to compare
against in the impulse tests, whose architecture otherwise defines
`FAUSTFLOAT double` (as `tests/impulse-tests/archs/impulseexecopts.cpp` does
in faust-rs too). A SIMD path is validated in one precision pair at a
time, and the pair of the tests is not the pair of deployment.

### 1.3 The GPU path

`-gpu` selects `MojoGpuCodeContainer`. Read from `mojo_instructions_gpu.cpp`,
`prepare` and `writeLaunch`:

- the levels of the `-vec` loop DAG are taken in order; inside a level,
  the sub-loops (tasks) are sorted by their text and batched greedily, a
  new stage being opened when a task's reads or writes intersect the
  reads or writes already in the batch;
- each stage is one kernel; inside it, `task = global_idx.x` and an
  `if task == j` chain selects the sub-loop; each thread runs its
  sub-loop over the samples of the chunk, sequentially, recursive state
  included;
- `gpu_compute` enqueues a controls kernel once, then, for every chunk of
  at most `-vs` frames, every stage kernel in order, then an optional
  post kernel; the architecture then copies outputs and passive zones
  back and synchronises before returning from the audio callback;
- `block_dim` is `min(32, tasks)`, so the grid has as many threads as the
  stage has tasks;
- `-single` is required, and the architecture requires `f32` samples;
  DSP fields must be device-copyable scalars or fixed-length inline
  arrays, pointer views being rebuilt inside each kernel.

Temporary fields shared between tasks live in a `mydspWork` structure on
the device; the DSP structure is copied to the device once, active UI
zones are copied when they change, passive zones are read back. The
architecture side is clean: `FaustDspGpu` extends `FaustDsp`, an
`AdapterDsp` presents the device as an ordinary `FaustDsp` to the
unchanged PortAudio driver, a control map is built by visiting the
generated `build_user_interface` with a classifying GUI.

The pull request gives no GPU measurement and calls the path
experimental. Its structure says why there will be no good one:

- **the parallel axis is the loop DAG of one instance.** A freeverb has a
  few dozen sub-loops, a filter has three. On a device built for tens of
  thousands of threads, the occupancy is near zero;
- **each thread is sequential in time.** A `~` recursion orders the
  samples; the thread walks its chunk one sample at a time, as a slow CPU
  core would;
- **launches dominate.** A block of 512 frames with chunks of `-vs = 4`
  and S stages is 128·S kernel launches plus one synchronisation per
  callback; the fixed cost of a launch is larger than the work.

This is the task parallelism of `-omp` and `-sch`, transposed to a GPU. It
makes sense on a few CPU cores; on an accelerator it is the wrong grain.
The path does not use the one classification that would help it, the
recursive/non-recursive split of the sub-loops (§3.4).

## 2. What faust-rs has in front of each piece

- **Benchmark.** Nothing comparable. `xtask` has `compile-budget`,
  `compile-profile`, `examples-compare`, the diff reports between
  backends, and the `cost_probe` test of `cranelift-ffi`; `faustprobe
  --time` measures one program against real time on Cranelift; the memory
  of the project records that the compile-budget gate cannot run on this
  machine and that pre/post throughput comparisons are done by hand. The
  impulse runner covers 133 programs on 8 backends and 2 vector variants
  for correctness, not for throughput, but it knows how to build and run
  every backend, which is most of what a benchmark needs.
- **Vector mode.** The signal-level port is complete: `-vec` with the loop
  DAG (`ComputeMode::Vector`, `-vs` defaulting to 32 as in C++, the two
  `-lv` loop variants), `-ss` authoritative, 98 programs certified in all
  16 modes, and the lockstep *instance* vectorization (section 8 of the
  vector-mode plan), which C++ Faust does not have: K isomorphic instances
  detected as a bundle, the isomorphism verified by a second traversal
  with an explicit leaf mapping, the bundle lowered as one physical sample
  loop whose expressions carry lanes. The `lockstep-simd-check` gate of
  `xtask` then requires that clang at `-O3` turns those lanes into LLVM
  vector operations, attributed by line table to the generated loop. So
  the SIMD is clang's, as in C++ Faust; faust-rs emits scalar lane
  expressions. There is no `-omp` and no `-sch`.
- **Backends.** `c`, `cpp`, `rust`, `wasm`, `julia`, `interp`,
  `cranelift`, sharing a C-family emitter core for `c`/`cpp`. Two of them
  have no autovectorizer at all: Cranelift has no loop vectorization pass,
  and the WASM path depends on the engine. This is exactly Mojo's
  situation in §1.2.
- **GPU.** Nothing.
- **Differentiation.** `fad` (forward, lanes), `rad` (reverse,
  `BlockReverseAD` with a tape sized per block), the block augmentation
  through clocked wrappers for `fad` only; `optimizers.lib` with
  `multistart_1D`, `grid_then_descend_1D`, the (1+1)-ES, SPSA, all of which
  copy the graph K times with `par`; `faustprobe --train` with `--sweep`
  as a grid of starting points, run one after the other, and `--blocks`
  with a cleared state per block as an offline calibration, one epoch per
  block.

## 3. The parallel axis: instances, not the DAG

### 3.1 The batch

A GPU wants thousands of threads running the same code with no dependency
between them before a final reduction. Inside one Faust instance that
structure does not exist: the sub-loops are few and ordered, and the
recursions order the samples. Between instances it does. "N copies of the
same program with different leaves, each with its own state" is the batch
in the DDSP sense, and faust-rs already produces it in several places:

- `multistart_1D` and `grid_then_descend_1D`, one copy per starting point;
- the ES population and the two evaluations of an SPSA step, copies with
  perturbed parameters;
- `faustprobe --train --sweep`, a grid of starting points, sequential
  today;
- `--train --blocks` with a cleared state per block, where every block
  could be an instance;
- a dataset of examples for an offline calibration, each example an
  instance.

The lockstep bundle is this structure seen by the compiler, and its
isomorphism certificate ("these K instances are the same program") is
exactly what makes one kernel valid for all of them. Today the lane count
is the SIMD width, 4 or 8, and clang vectorizes across lanes. On a GPU the
lane is the thread.

### 3.2 What the compilation would look like, compiler-side

The kernel is the *scalar* `compute` of one instance over the whole block,
with no stage decomposition; the thread index is the instance index. The
state becomes a structure of arrays indexed by instance; shared controls
are broadcast; per-instance parameters (the trained ones, or the starting
point) are arrays; each instance writes its loss into an array that the
host reduces, or that a second kernel reduces on the device. One launch per
block, or per epoch when the block is the dataset.

Reusable from the pull request, at the architecture level: the device copy
of the DSP structure, the control map built by visiting
`build_user_interface`, the active/passive zone transfers, the work
buffer, the adapter that presents the device as an ordinary DSP. Not
reusable: the visitor, which has no tasks to order any more, and the fixed
K of a bundle, which must become a run-time size.

### 3.3 The cheaper form: replication in the architecture

The structure-of-arrays form of §3.2 needs the compiler to split the
instance structure by field. There is a form that needs no compiler change
beyond an emitter: the architecture allocates N instance structures side by
side on the device (an array of structures), the kernel is the scalar
`compute` of one instance with `dsp = &instances[thread]`, and per-instance
parameters are simply the control fields of each structure, which already
exist. The only compiler-side requirement is a device-compatible scalar
emitter: no pointers in the state, fixed-length inline arrays, views
rebuilt in the kernel, which is precisely the constraint the pull request
states for its own device path (§1.3). So the batch axis does not need the
lockstep bundle at all to exist; it needs it for two refinements:

- **memory coalescing.** In the array-of-structures form the threads of a
  warp read the same field at a stride of one structure, which wastes
  bandwidth; the structure-of-arrays form of §3.2 makes them read
  consecutive addresses. That is what the compiler-side split buys, and
  it matters for delay lines and tables more than for scalar state;
- **the CPU widening.** On a CPU the lanes must be in one loop body for
  SIMD to apply, and that is the lockstep lowering as it exists, widened
  from 4 or 8 lanes to 16 (AVX-512) or to several threads sharing the
  lanes.

Divergence is the price of one program on many threads: a `select2`
lowered as a branch, a per-instance clock of an `ondemand` block, an
`iterate` count that differs per instance, all make threads of a warp take
different paths, which the hardware serialises. It works and it is slower;
the numerical semantics is unchanged.

### 3.4 The second dimension: time, for the sub-loops without recursion

The `-vec` classification already marks which sub-loops are recursive and
which are independent across samples. An independent sub-loop over a block
of T samples is a kernel of T threads, fixed-delay reads and table reads
included, and with the instance axis this is a two-dimensional grid,
instance × time. The intermediate values between sub-loops are then
materialised per instance and per sample, which the `-vec` mode already
does on the CPU with its vector buffers of `-vs` samples; on the device
they become arrays of T. The recursive sub-loops keep one thread per
instance and walk the T samples. The pull request has this classification
in hand and does not use it.

A recursion through a delay of D samples, `y = x + a * y@D` (a comb, a
Karplus-Strong string, a feedback delay network with long lines), is D
independent chains: the samples of each residue class modulo D depend only
on each other. Within a block of T samples that is D threads doing T/D
sequential steps each, a parallelism that neither the DAG nor the
instance axis exposes. Whether the vector-mode classification of faust-rs
distinguishes a recursion by its minimum delay, as the C++ `-vec` does with
`-mcd` for the choice between copies and ring buffers, is to check before
counting on it; the property of the signal is there regardless.

### 3.5 Real-time uses of the instance axis

Training and calibration are the first use, but the instance axis has two
real-time ones, which the DAG axis does not:

- **polyphony.** Sixty-four voices of one instrument are sixty-four
  instances with their own state and their own controls, exactly the
  `poly` architecture of C++ Faust, and a batch by construction;
- **many channels.** Spatialisation, wave-field synthesis over hundreds
  of loudspeakers, ambisonic decoding, a filter bank per channel: the same
  program on many channels is the same batch.

The latency caveat stands: one transfer to the device and one back per
block, plus a synchronisation. Consumer devices have a round trip of the
order of a millisecond, so a block of 256 frames at 48 kHz (5.3 ms) is
possible and a block of 32 is not; and all instances go in one transfer,
so the cost does not grow with the voice count. This is the axis the
commercial real-time GPU audio engines use, channels and voices, not the
loop graph of one effect.

### 3.6 Limits to state in advance

- For training, the transfer per block and the synchronisation are not a
  problem; for real time they bound the block size (§3.5).
- A GPU thread on sequential scalar code is ten to fifty times slower
  than a CPU core; the gain appears from a few hundred instances up, or
  from a few dozen when the time axis of §3.4 applies too.
- Consumer GPUs compute in `f32`, as the pull request's `-single`
  requires; `f64` costs a large factor except on data-centre parts.
- The state memory is multiplied by N, which matters for long delay lines
  and tables.
- `fad` per instance is natural, one tangent lane per thread. `rad` per
  instance needs one tape per thread, N × T × the tape width, and `rad`
  does not cross the clocked wrappers today.

An intermediate step avoids all of it for the common sizes: widen the
lockstep on the CPU, sixteen lanes with AVX-512 or several threads
sharing the lanes. That covers the K of `multistart` and the grids of
`faustprobe` up to a few dozen instances without writing a GPU emitter.
The GPU only becomes necessary above that, for a whole dataset, a large
ES population, or the voice counts of §3.5.

## 4. How the classical DDSP systems occupy a GPU, and how near faust-rs can get

### 4.1 The four axes of the DDSP systems

- **The batch.** B examples processed together, typically 16 to 64 clips of
  a few seconds. Always present.
- **Time, by choosing operations without recursion.** The original DDSP
  (Engel et al., 2020) is designed for this: the harmonic oscillator gets
  its phase by a cumulative sum, a parallel scan, then one sine per
  sample; the filtered noise goes through FFT and overlap-add; the
  reverberation is an FFT convolution. No sample depends on the previous
  one through a loop, so T threads on the time axis. `torchsynth` (Turian
  et al., 2021) goes further in the same direction, a modular synthesizer
  whose every module is chosen to be free of per-sample recursion so that
  a batch of voices renders thousands of times faster than real time.
- **Frames.** The neural network that produces the controls runs at frame
  rate, one frame every few milliseconds, as matrix products over batch ×
  frames × channels (§4.5).
- **Channels and parameters.** The layers' matrices, where a GPU is at
  home.

Recursive filters are the sore point, and the literature answers it three
ways. Avoid the recursion: the differentiable biquads of Nercessian (2021)
are evaluated through their sampled frequency response, hence by FFT.
Exploit linearity: a linear recurrence, even with time-varying
coefficients, is associative and is computed by a parallel scan in
logarithmic depth, which the state-space models (Mamba, LRU) made
standard. Accept the sequence: `torchlpc` and the time-varying IIR filters
of Yu and Fazekas keep a per-sample loop inside a CUDA kernel, one thread
per batch element and channel, with a hand-derived backward pass that is
itself a filtering; the RNN virtual-analog models do the same, sequential
in time, parallel over the batch, with truncated sequences and the state
carried from one segment to the next.

The gradient is reverse mode everywhere, backpropagation through time,
memory proportional to T, checkpointing when it overflows.

### 4.2 Axis by axis

| axis | classical DDSP | faust-rs |
|---|---|---|
| batch | B examples | instances, by architecture (§3.3) or by lockstep (§3.2) |
| time, non-recursive loops | by design of the synthesizers | the `-vec` DAG already classifies them (§3.4) |
| linear recursions | scan or FFT, chosen by hand | detectable by the compiler |
| nonlinear recursions | sequential per batch element | the same; nothing better exists |
| frames and matrices | GEMM | not produced (§4.5) |

**The batch** is §3, reachable with what exists.

**Time without recursion** is already known to the compiler (§3.4).

**Linear recursions** are where faust-rs has a lever the DDSP systems do
not. Where a PyTorch user must decide to write a biquad in the
frequency-sampling form, the compiler sees the `~` and can decide whether
the feedback is linear. The FIR/IIR reveal project
(`porting/fir-iir-reveal-activation-plan-2026-07-20-en.md`, carriers and
algebra in place, reveal producer unported) goes exactly there. Two
nuances on what the two evaluations give:

- **by FFT, for a time-invariant recursion.** Evaluating an IIR over a
  block by FFT convolution with its truncated impulse response is an
  approximation, with an error that decays as the response does, hence
  slowly for a high-Q filter; and the state at the block boundary must be
  carried, which means adding the zero-input response computed from the
  state-space form. The revealed IIR is exactly that form (A, B, C, D), so
  the carry is available; the truncation is not, and it is the same
  approximation the frequency-sampling biquads accept explicitly;
- **by scan, for a linear recursion with time-varying coefficients.** A
  first-order recurrence `y = a·y' + b` composes as affine maps,
  `(a₂, b₂) ∘ (a₁, b₁) = (a₁a₂, a₂b₁ + b₂)`, and an order-k one as k×k
  matrices; the scan is exact up to rounding, in a different summation
  order from the sequential form, which is to be qualified against it as
  the impulse tests qualify everything else, at their tolerance and with
  attention to `f32` near a pole at |a| → 1.

The choice between the two, and between them and the sequential form,
becomes a compiler pass with its independent checker, not a rewrite of the
model.

**Nonlinear recursions**, the Newton solver of section 2.5 of the
optimizers overview, a saturation in a feedback loop, the virtual-analog
models, stay sequential in time. Nobody does better, `torchlpc` included.
Parity by construction, with the batch as the only axis, and the block
splitting of `--train` with the carried state playing the role of the
truncated sequences.

**Frames and matrices** are not produced, and §4.5 says why. Neural DDSP,
with an encoder that listens and a decoder that drives, is not the
target.

### 4.3 The gradient changes nature

The DDSP systems need reverse mode because they train hundreds of thousands
of weights. A parametric Faust model has five to fifty parameters. Below a
few dozen parameters, forward mode by lanes costs as much as reverse mode
and has no memory in T. That is why "`fad` only" through the clocked
wrappers is not the handicap it would be in deep learning.

### 4.4 What can be hoped for, and the shape of the bridge

On the parametric part, physical models, filters, virtual analog: parity of
throughput with the custom CUDA kernels of the literature, plus the
automatic reveal of the linear structures. No more, and not on the neural
part. The realistic way to profit from it is not to rebuild a DDSP in
Faust but to give the DDSP systems the piece they lack: a Faust kernel
compiled on the batch axis, with its derivative, exposed as a PyTorch
autograd operation, as `torchlpc` is for all-pole filters. The network
stays on the Torch side, the signal model on the Faust side.

The bridge has two shapes, decided by where the Faust block sits:

- **the Faust block first, fed by a recording.** What Torch needs is
  dL/dp for the P parameters of the block, given dL/dy over the T output
  samples. P tangent lanes of `fad` give the P columns dy/dpᵢ in one pass,
  and dL/dpᵢ is the dot product of dL/dy with the column: forward mode,
  no tape, the batch axis for the examples;
- **the Faust block fed by a network.** Torch then also needs dL/dx over
  the T input samples, a vector-Jacobian product with T columns, which
  forward mode cannot give at any reasonable cost. That is `rad`, one
  tape per instance, and the reason the reverse mode of faust-rs matters
  even with few parameters: not for the parameters, for the input.

The first shape is the calibration of a model against a recording and
needs nothing that faust-rs does not have, except the batch axis. The
second is the neural DDSP with a Faust synthesizer, and it needs `rad`
through everything the model uses, clocked wrappers included.

### 4.5 GEMM, and why Faust does not produce one

GEMM, *general matrix multiply*, is the BLAS routine `C = α·A·B + β·C`
with A of size M×K and B of size K×N (`sgemm`, `dgemm`). It is the
operation a GPU is built around because it does M·N·K multiply-adds for
only M·K + K·N + M·N numbers read or written: the ratio of computation to
memory traffic grows with the size, the compute units stay busy instead of
waiting for memory, and the devices carry dedicated units (tensor cores)
for it. A well-written GEMM reaches a large fraction of the peak; scalar
sequential code reaches a small one. Deep learning reduces to it: a dense
layer over a batch is a GEMM with the inputs as A and the weights as B, a
convolution is rewritten as one, attention is two. The control network of
a DDSP, from frames to synthesizer parameters, is made of nothing else.

Faust can write a dense layer:

```faust
dense(N, M, w) = si.bus(N) <: par(j, M, (par(i, N, *(w(i, j))) :> _));
```

The compiler unrolls `par` and `:>` into N·M scalar multiplications written
one by one, with the weights as constants or controls: 65 536 lines for a
256 × 256 layer, no loop, no library call, no tiling for the cache. It works
for a small layer and is unreasonable well before the sizes of a network.
Nothing in the language tells the compiler "this is a matrix" and nothing
in the backends emits a BLAS call or a tiled kernel.

Two nuances keep this honest. The isomorphism detection of the lockstep
could in principle recognise `par(i, N, *(w(i)))` as a matrix-vector
product, the same way it recognises K copies of a program; but a
matrix-vector product per sample is memory-bound, and a GEMM only appears
with a second axis, the batch of §3 or the frames of an `ondemand` domain.
So "out of reach" is more exactly "not produced, and not the axis on which
faust-rs has anything to offer": the layer belongs to the Torch side of the
bridge of §4.4.

## 5. What is worth taking, in order

1. **The benchmark, first.** An `xtask bench` over the matrix backends ×
   modes (`c`, `cpp`, `rust`, `cranelift`, `interp`, `wasm`, `julia`;
   scalar, `-vec` at several `-vs` since `-vs 4` is what made the C++
   `-vec` slow in §1.1, lockstep), one CSV row per case with the output
   checksum, snapshots, a plot; built on the impulse runner, which already
   compiles and runs every backend, and on `faustprobe --time` for
   Cranelift. What it must control and record: the block size, a warm-up,
   the minimum and maximum of N repetitions, the compiler flags (no
   fast-math, contraction stated), denormals (`-ftz`), and the platform,
   as the `PLATFORM` file of the `faustprobe` snapshots does. It makes every
   generation change measurable and fills the gap the memory records. It
   follows the phase methodology: the harness before the change.
2. **Explicit SIMD, but for Cranelift and WASM, not for C++.** Cranelift has
   no loop autovectorizer, WASM has `simd128`; today the lockstep depends
   on clang. A FIR lowering with `SimdLoad` / `SimdStore` / `SimdOp` nodes
   carrying a lane width, produced from the lockstep bundle and from the
   affine sub-loops of the `-vec` DAG, would serve both and make the
   lockstep independent of clang. A second benefit: explicit nodes carry
   an explicit contraction policy, where today clang contracts
   multiply-adds within a statement on arm64 and Cranelift does not (the
   journal of this day, section 2.9 of the `iterate` plan), so the
   bit-level agreement between backends would stop depending on the host
   compiler. As a FIR pass with an independent checker and reverting
   mutations; without the pull request's couplings by name (`vindex`,
   bargraph fields), without the "last store decides" assumption, with a
   tail loop, and validated in both precision pairs (§1.2).
3. **The batch axis, by the architecture first** (§3.3): a
   device-compatible emitter and N instance structures side by side, with
   `faustprobe` filling the per-instance controls and reading the
   per-instance losses; measured by the benchmark of item 1; the
   structure-of-arrays split and the CPU widening of the lockstep after,
   when coalescing or SIMD width is what limits.
4. **The Mojo backend itself, not now.** The C-family emitter core makes a
   new text backend cheap, but Mojo brings nothing in scalar (§1.1) and
   adds a toolchain dependency through Pixi. It would become interesting
   only if the GPU target goes through Mojo rather than WGSL or CUDA.

## 6. Decisions open before any plan

- Whether the benchmark measures throughput only or also the compile
  budget, which has its own gate that cannot run here.
- The lane width model of the SIMD FIR nodes: fixed by the target
  (Cranelift's `f32x4`, WASM's `v128`) or a parameter of the pass with a
  scalar fallback, as `-vs` is today.
- Whether the run-time N of an instance batch is an architecture matter
  only (§3.3) or also a compiler surface (a structure-of-arrays mode,
  §3.2), and in which order.
- The GPU emitter. On the machine of record, an Apple silicon Mac, the
  only device is Apple's through Metal, so the choices are WGSL through
  `wgpu` (portable over Metal, Vulkan and DX12, `f32` only), Metal
  Shading Language directly, or Mojo (the pull request's runtime, one more
  toolchain); CUDA (`f64` possible, NVIDIA only) cannot be tried here.
- Whether the PyTorch autograd bridge of §4.4 is a `faustprobe` mode, a
  crate, or an architecture file; and which of its two shapes comes first
  (the `fad` shape needs only the batch axis).
- Whether the vector-mode classification distinguishes a recursion by
  its minimum delay (§3.4), to be read in `signal_fir/vector/plan` before
  the time axis is planned.

## 7. References

- Pull request 1293 of `grame-cncm/faust`: benchmark framework, Mojo
  backend, explicit SIMD, experimental GPU (manuelfarzini, 2026).
- J. Engel, L. Hantrakul, C. Gu, A. Roberts, "DDSP: Differentiable Digital
  Signal Processing", ICLR 2020.
- J. Turian et al., "One billion audio sounds from GPU-enabled modular
  synthesis" (`torchsynth`), DAFx 2021.
- S. Nercessian, A. Sarroff, K. J. Werner, "Lightweight and interpretable
  neural modeling of an audio distortion effect using hyperconditioned
  differentiable biquads", ICASSP 2021.
- C.-Y. Yu, G. Fazekas, "Differentiable all-pole filters for time-varying
  audio systems" (`torchlpc`), DAFx 2024.
- A. Gu, T. Dao, "Mamba: linear-time sequence modeling with selective state
  spaces", 2023; A. Orvieto et al., "Resurrecting recurrent neural
  networks for long sequences" (LRU), ICML 2023: parallel scan of linear
  recurrences.
- G. E. Blelloch, "Prefix sums and their applications", 1990.
- B. Christianson, "Reverse accumulation and attractive fixed points",
  Optimization Methods and Software, 1994 (the derivative of a contractive
  iteration, section 2.5 of the optimizers overview).
- In this repository: `porting/vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`
  (section 8, lockstep); `porting/fir-iir-reveal-activation-plan-2026-07-20-en.md`;
  `porting/iterate-counted-loop-with-exit-analysis-and-plan-2026-09-18-en.md`
  (section 2.9, contraction); `libraries/optimizers-overview-en.md`
  (section 2.5).
