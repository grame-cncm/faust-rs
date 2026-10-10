# Proposal: `nffunction`, an external function with multiple outputs

Date: 2026-10-08

Status: **language and compilation proposal, not implemented**. Examples using
`nffunction` do not yet compile with either `faust-rs` or the reference Faust C++
compiler. The proposed name is `nffunction`; `ffunction` keeps its current
contract.

French version: [nffunction-multiple-outputs-proposal-2026-10-08-fr.md](nffunction-multiple-outputs-proposal-2026-10-08-fr.md).
Keep both versions synchronized.

## 1. Motivation

`ffunction` describes an external function with N scalar arguments and a single
scalar result. For an FFT, a frame operator, or a function that produces several
related values, we would instead like an **N→M** operator: one call consumes
N values and produces M results.

The proposal introduces one external call shared by all outputs. The compiler
prepares the arguments, calls the function, and exposes its results. For a
frame-based FFT, `ondemand` provides the call rate.

The goals are:

- a Faust N→M signature, with independent input and output types;
- compact notation for groups of values of the same type;
- a C ABI with two pointers, even for large N and M;
- one call shared by the M outputs, executed in its clock domain;
- no user-level protocol of tokens, external writes, or external reads.

## 2. Proposed Faust syntax

### 2.1 Scalar types

```faust
op = nffunction(
    (int, float, float) foo_f32|foo_f64|foo_f80|foo_fixed(float, int, float),
    <foo.h>, ""
);
```

The list before the name describes the outputs; the list after the name
describes the inputs. This operator has three inputs and three outputs:

| Position | Input | Output |
|---|---|---|
| 0 | `float` | `int` |
| 1 | `int` | `float` |
| 2 | `float` | `float` |

Symbol variants, the header file, and the library specification follow the
`ffunction` convention. These names are illustrative: each variant actually
used must exist in the external library. The first implementation can target
`-single` and `-double`; unsupported precision modes must produce an explicit
diagnostic.

### 2.2 Type repetition

```faust
fft1024 = nffunction(
    (float[1024]) fft1024f|fft1024d|fft1024l|fft1024fx(float[1024]),
    <fft.h>, ""
);
```

In an `nffunction` prototype, `float[K]` means **K consecutive scalar ports of
type `float`**. It is repetition within the signature; it does not introduce an
array-valued signal into the Faust language.

Groups are concatenated in declaration order:

```faust
op = nffunction(
    (int, float[1024]) foo_f32|foo_f64|foo_f80|foo_fixed(float[1024], int),
    <foo.h>, ""
);
```

This signature describes 1025 inputs and 1025 outputs. The integer input is the
last port; the integer output is the first.

Size K must be a constant integer expression evaluated at compile time, strictly
positive and within the compiler's limits. For example, `float[N+2]` is valid
when N is known at compile time. A dynamic, negative, or zero size, or overflow
in the sum of arities, must be rejected. The first version is limited to `int`
and `float`.

### 2.3 Number of FFT outputs

The name `fft1024f` does not determine the spectrum format. The prototype must
match the C function's contract:

| Transform and layout | Scalar inputs | Scalar outputs |
|---|---|---|
| Real FFT, spectrum packed into N real values | N | N |
| Real FFT, N/2+1 complex values with interleaved real/imaginary parts | N | N+2 |
| Full complex FFT, interleaved complex values | 2N | 2N |

The `fft1024` example above assumes a real spectrum packed into 1024 values,
with its layout documented in `fft.h`. For a real spectrum with 513 interleaved
complex values, the declaration would be:

```faust
rfft1024 = nffunction(
    (float[1026]) rfft1024f|rfft1024d|rfft1024l|rfft1024fx(float[1024]),
    <fft.h>, ""
);
```

Sign convention, normalization, and complex layout belong to the external
function's contract. The compiler does not infer them from its name.

## 3. Use with `ondemand`

### 3.1 Inputs already available in parallel

```faust
si = library("signals.lib");
il = library("interleave.lib");

fft1024 = nffunction(
    (float[1024]) fft1024f|fft1024d|fft1024l|fft1024fx(float[1024]),
    <fft.h>, ""
);

// 1024 inputs, 1024 outputs; one trigger every 1024 samples.
process = (il.frame_clock(1024), si.bus(1024)) : ondemand(fft1024);
```

For `F : N→M`, `ondemand(F) : N+1→M`: the first input is the clock, and the
next N inputs are the data. Here the clock is boolean. At each trigger:

1. the N input values are sampled;
2. one external call computes the M results;
3. the M results become the block's new outputs.

Between triggers, the outputs remain constant. Before the first trigger, they
are zero, with each port's respective type. The call and buffer filling must
remain inside the guarded block.

The guarantee is **one call per activation of a live node**, shared by all its
outputs. A node with no used outputs can be eliminated because it is pure.
Without `ondemand`, the operator is evaluated at its domain's rate, ordinarily
the audio rate.

`ondemand` also accepts clocks interpreted as counts: the general guarantee is
one call per internal activation, not necessarily one call per audio sample.
The FFT examples use only 0 or 1.

### 3.2 One audio input and a 1024-sample window

```faust
il = library("interleave.lib");
si = library("signals.lib");

fft1024 = nffunction(
    (float[1024]) fft1024f|fft1024d|fft1024l|fft1024fx(float[1024]),
    <fft.h>, ""
);

process = il.serialize_in(1024)
        : (il.frame_clock(1024), si.bus(1024))
        : ondemand(fft1024);
```

The history is built **outside the `ondemand` domain**, at the audio rate. The
first trigger occurs at t=1023: the window then contains `x[0]…x[1023]`, in that
order. The external function is subsequently called once every 1024 samples.

With a 1024-sample window and a hop of 256, only the clock changes:

```faust
process = il.serialize_in(1024)
        : (il.frame_clock_hop(1024, 256), si.bus(1024))
        : ondemand(fft1024);
```

This clock starts triggering at t=255. The first windows are padded with zeros
from the initial history; they do not yet contain 1024 received samples. Waiting
for the first full window would require an additional clock condition.

**Window collection cannot be moved inside this `ondemand`.** A function called
once every 256 samples and receiving only the current sample cannot recover the
255 intervening samples. The `process_sample(x)` model, called at every tick
with an internal buffer and counter, has a different contract: a stateful
external operator.

### 3.3 Returning to an audio stream

For a frame operator `FX : N→N`, the existing construction remains:

```faust
process = il.interleave(1024, FX);
// Or, for overlapping frames:
// process = il.interleave_hop(1024, 256, FX);
```

`FX` can compose an FFT `nffunction`, Faust spectral processing, and an inverse
`nffunction`. An analysis-only FFT exposes held bins; it must not feed directly
into `interleave` if its output arity differs from N. An analysis/synthesis
window and its normalization are still required for overlapping reconstruction.

`nffunction` removes the external transfer protocol, but does not replace
`serialize_out`: stream reconstruction cost is a separate issue.

## 4. Proposed C ABI

### 4.1 Homogeneous signature: two buffers

For `float[1024]→float[1024]`, the recommended C contract is:

```c
void fft1024f(const float* inputs, float* outputs);
void fft1024d(const double* inputs, double* outputs);
```

There are **two C parameters**, rather than 2048. Sizes are fixed by the Faust
prototype and known to the external function. The selected symbol depends on
the active precision; `float` in the Faust prototype denotes the real
computation type, as with `ffunction`, rather than invariably C `float`.

For homogeneous integer inputs, the buffer uses `int32_t`. Input and output
buffer types may differ. An entirely homogeneous list is transported in one
contiguous array, even if several groups of that type appear in the prototype.

### 4.2 Heterogeneous signature: two structures

For `(float, int, float)→(int, float, float)` in single precision:

```c
#include <stdint.h>

typedef struct {
    float g0;
    int32_t g1;
    float g2;
} foo_f32_inputs;

typedef struct {
    int32_t g0;
    float g1;
    float g2;
} foo_f32_outputs;

void foo_f32(const foo_f32_inputs* inputs, foo_f32_outputs* outputs);
```

In a heterogeneous list, each prototype group becomes a field `g0`, `g1`, etc.,
in declaration order. A repeated group becomes an array field:

```c
typedef struct {
    float g0[1024];
    int32_t g1;
} grouped_f32_inputs;

typedef struct {
    int32_t g0;
    float g1[1024];
} grouped_f32_outputs;

void grouped_f32(const grouped_f32_inputs* inputs,
                 grouped_f32_outputs* outputs);
```

In double precision, real fields become `double`, and the symbol and type names
correspond to the double variant. Structures retain their natural C alignment;
casting a buffer of real values to a heterogeneous structure is not allowed.

The external header is authoritative for typedefs and prototypes. The Faust
compiler must emit references to the agreed names without redefining these
structures. An auxiliary header description or generation tool could avoid
manual declarations; this is a complementary tool, not additional syntax
required for the MVP. A declaration mismatch must be detectable by the C/C++
compiler, without pointer conversions that conceal it.

### 4.3 Lifetime and constraints

Buffers belong to the generated DSP or its invocation; they are not shared
globals. The function:

- only reads inputs and writes every output element;
- does not retain the pointers after returning;
- completes synchronously before its outputs are used;
- respects sizes and does not access memory outside the buffers;
- assumes no aliasing between input and output buffers;
- produces the same results for the same inputs in a given computation mode,
  with no observable dependency on hidden state.

The prototype alone does not guarantee a particular SIMD alignment. An FFT
requiring it must use an adapter or an explicit alignment contract. FFT plan
preparation and expensive allocations must not occur at every audio activation.
Managing them requires a separate lifecycle contract, outside the pure MVP
described here.

## 5. Compilation: from syntax to generated code

The path follows the project's pipeline:

```text
parse → boxes → eval → propagate → normalize → type/interval
      → transform → FIR → backend
```

### 5.1 Parser, boxes, and evaluation

The parser recognizes the new keyword and two lists of typed groups. It retains
sizes as expressions until constant evaluation. A canonical builder and matcher
carry the primitive descriptor: input/output groups, name variants, header, and
library.

Evaluation resolves sizes and validates types, arities, and overflow. The
descriptor keeps groups compact: `float[1024]` does not become a chain of
1024 type declarations.

### 5.2 Propagation and signal representation

Propagation builds **one call node with multiple results**, owning its N
arguments, and M typed projections of that same node:

```text
ExternalCall(descriptor, arguments, domain)
 ├─ Projection(0) → output type 0
 ├─ Projection(1) → output type 1
 └─ …
```

Arguments remain scalar Faust signals. Each projection knows its index and
type; it does not embed a copy of the entire computation. The call and the data
defining its contract remain co-localized in the node, without an undocumented
side table.

Normalization must preserve the call/projection relationship. A call present
in several domains must not be shared across those domains. If sharing two
equivalent pure calls is allowed, it stays within the same domain and a
compatible execution position; it never crosses a guard.

### 5.3 Types and intervals

Each port has its own type. Argument conversion rules follow `ffunction`;
an incompatible type that cannot be converted is rejected before emission.

Without an additional external function contract, output intervals are
conservative. The compiler must neither invent an FFT bound nor infer the
absence of NaN or infinity. Derivatives are not inferred: FAD/RAD require a
dedicated rule; without one, differentiating this primitive must produce an
explicit diagnostic.

### 5.4 Scheduling and FIR

Lowering materializes the call as an operation producing several values, with
an explicit order:

```text
evaluate arguments
→ write input storage
→ call the external function
→ make outputs available
```

Reading a projection depends on the producing operation. A C `void` call that
writes a buffer is not a separate scalar expression for each output. FIR must
be able to express typed storage, its addresses, the call, and read dependencies;
any missing capabilities must be added explicitly.

Under `ondemand`, these operations are emitted in the existing guarded region.
Storage for held outputs persists between activations and is zeroed by
`instanceClear`. Temporary inputs do not need to persist after the call. For
large sizes, the backend favors reusable per-instance storage over large
automatic arrays on the stack.

FIR can retain the logical node's purity while expressing the local writes
required by the ABI. CSE and scheduling passes must respect these dependencies;
a function name and its arguments alone are insufficient to represent its
materialized C call.

### 5.5 Generated C sketch

The following sketch shows a 1024→1024 call in single precision. `window`
contains the already assembled window in chronological order; `fire` is the
external clock. This code illustrates the sequence, not the exact emission of
Faust delay lines.

```c
#include <string.h>
#include <fft.h>

typedef struct {
    float foreign_inputs[1024];
    float held_outputs[1024];
} spectral_state;

void spectral_clear(spectral_state* state)
{
    memset(state->held_outputs, 0, sizeof state->held_outputs);
}

void spectral_tick(spectral_state* state, int fire, const float window[1024])
{
    if (fire != 0) {
        for (int i = 0; i < 1024; ++i) {
            state->foreign_inputs[i] = window[i];
        }
        fft1024f(state->foreign_inputs, state->held_outputs);
    }
    /* Projections read state->held_outputs after this block. */
}
```

Writing directly to held storage is possible here because the return is
synchronous and all outputs are defined. If lowering uses a temporary output
buffer, its copy into held values stays inside the guard. A header used from
C++ must provide the usual `extern "C"` guards for a C library.

### 5.6 Compilation and linking

After implementation, the Faust command would use the usual options:

```sh
faust-rs -single -I libraries -lang cpp fft_analysis.dsp -o fft_analysis.cpp
```

The subsequent C++ compilation of the DSP and its architecture must find
`fft.h` and link the adapter and its FFT library, such as FFTW. Selecting
`-double` must select the double variant and `double` buffers. The file produced
by `-lang cpp` is a DSP to integrate into an architecture, not necessarily a
standalone executable.

These commands describe the intended use; they are not evidence of an executed
`nffunction` test. No new CLI flag is required for the MVP.

## 6. Expected gains and limitations

| Element | Expected effect |
|---|---|
| Chained external writes | Removed; the compiler fills input storage. |
| Separate external reads | Replaced by projections of one shared call. |
| C signature | Two pointers instead of N+M scalar parameters. |
| FFT core | Library call; butterflies are not unrolled into the Faust graph. |
| Frame transfer | O(N+M) per activation, excluding external computation. |
| Graph size at boundaries | N inputs and M projections remain represented; at least O(N+M) cost. |
| Audio history | Still required outside `ondemand`. |
| `serialize_out` reconstruction | Unchanged by this primitive. |

The `float[N]` notation reduces source and descriptor size, but does not make
total compilation cost constant. The compiler should be able to emit copy loops
when data is indexable; it cannot arbitrarily turn N distinct scalar
expressions into a single array read.

Removing scalar ports at the boundaries as well would require a buffer
representation in the graph, with collection and reconstruction operators. The
OLA work described in the performance plan remains applicable to replacing
reconstruction with a circular accumulator.

A function that modifies observable persistent state, uses a per-instance
context, or can fail requires a separate extension: a lifecycle contract,
initialization, reset, destruction, and error policy. These behaviors must not
be hidden inside the pure `nffunction` MVP.

## 7. Proposed implementation scope

The proposed MVP covers `int` and `float`, constant groups, multiple outputs,
the two-pointer ABI, C/C++ backends in single/double precision, and boolean
`ondemand` domains. Vector mode integration must preserve the guarded block and
its dependencies; an unvalidated mode must be explicitly rejected rather than
produce incorrect code.

Other backends require an explicit external linking and execution policy. The
interpreter or JIT cannot call an arbitrary symbol merely because a C header
appears in the source. Unsupported modes must diagnose the primitive.

Before implementation, the proposed syntax and ABI must be validated, along
with backend scope. Implementation follows crate boundaries:

| Layer | Work |
|---|---|
| `parser`, `boxes`, `eval` | Syntax, builder/matcher, compact descriptor, constants, and arities. |
| `propagate`, `signals`, `normalize` | Shared call, projections, and domain preservation. |
| `sigtype` | Per-port types and conservative intervals. |
| `transform`, `fir` | Scheduling, storage/addresses, call, and output holding. |
| `codegen` | ABI, headers, precision variants, and C/C++ emission. |
| `compiler` | Diagnostics, integration, and end-to-end validation. |

## 8. Validation and acceptance criteria

Tests should first use small deterministic external functions, without
depending on a local Faust installation or an FFT library. An actual FFT then
provides a complementary integration test.

1. **Signature and arity**: constant groups, heterogeneous inputs/outputs, port
   order, and diagnostics for invalid sizes and overflow.
2. **Multiple results**: a function produces several distinct values; all
   projections come from the same call. Inspect FIR/generated code to verify
   that the call is not duplicated per output.
3. **Clock**: no call on inactive ticks; one call per boolean trigger; initially
   zero outputs held between triggers.
4. **Isolation**: two distinct domains and two DSP instances share neither
   buffers nor results; reset conforms to the Faust lifecycle.
5. **ABI**: compile and execute generated C/C++ with the corresponding
   header/adapter, in single and double precision, for both storage forms.
   Check integer types and the absence of casts concealing a mismatch.
6. **FFT**: compare bins against a reference DFT at small sizes, then test
   analysis/inverse FFT with documented layout and normalization conventions.
   Check windows at trigger times.
7. **Optimizations**: identical results with and without optimization; no call
   moved outside its guard. Any call-count instrumentation is a testing tool,
   not an effect permitted by the pure contract.
8. **Cost**: measure compilation time, FIR/code size, and memory for N=256,
   512, 1024, 2048, and 4096; verify the removal of chaining and do not claim
   constant cost for the remaining scalar boundaries.

Before an implementation commit: run the `AGENTS.md` gates, particularly the
compilation budget, and update the registry of differences from the C++
reference. The public mapping would be an **extension**, not a 1:1 port. This
note does not declare any new syntax as already supported.

## 9. Repository references

- [Clock domains: `ondemand`, `upsampling`, `downsampling`](../docs/ondemand-note-en.md).
- [`interleave`, clock, and serialization definitions](../libraries/interleave.lib).
- [`interleave` semantics and phase convention](interleave-spectral-primitive-2026-07-07-en.md).
- [FFT scalability, CSE in guarded blocks, and reconstruction cost](fft-scalability-cse-in-clocked-blocks-2026-07-09-en.md).
- [Pure Faust frame-based FFT tests](../crates/compiler/tests/interleave_fft.rs).
- [Current `ffunction` decoding and lowering](../crates/transform/src/signal_fir/module/core_lowering.rs).
- [Registry of differences from Faust C++](faust-rs-vs-faust-cpp-differences-en.md).
- [Project rules](../AGENTS.md).
