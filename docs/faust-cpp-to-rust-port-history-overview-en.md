# From Faust C++ to `faust-rs`: a concise porting history

> This is the short, reader-oriented history of the project. For the complete
> chronology, subsystem details, dates, and source references, see the
> [full technical porting history](faust-cpp-to-rust-port-history-en.md).

## Purpose and outcome

The `faust-rs` project began in February 2026 with two related goals. The first
was to rebuild the Faust compiler in Rust while preserving the semantics and
public contracts of the production C++ implementation. The second was to test,
on a compiler of significant size, whether AI agents could contribute
effectively to a port involving not only translation, but also cleanup,
simplification, re-architecture, systematic validation, and new features.

Faust was a demanding subject for that experiment. Its implementation had
accumulated more than two decades of interactions between parsing, compile-time
evaluation, tree rewriting, signal typing, temporal state, code generation,
runtime lifecycle, architecture files, public C and C++ APIs, and many target
languages. A successful port could not be produced by translating one file at
a time. The real behavior first had to be reconstructed, divided into stages,
and surrounded with executable references.

The project reached its first complete source-to-code path after four days,
initially for a deliberately narrow subset and with almost all work executed at
sample rate. Roughly two months after the repository was created, it had an
operational scalar compiler comparable to official scalar Faust C++ on the
tracked portable corpus. Later work expanded the public APIs, executable
backends, WebAssembly integration, automatic differentiation, clock domains,
and checked vector compilation.

Rust was valuable for more than memory safety. Enums and exhaustive matching
made compiler vocabularies explicit; ownership exposed hidden global coupling;
small copyable IDs made tree identity cheap; `Result` and structured
diagnostics replaced implicit failure paths. Cargo provided one build, test,
documentation, and dependency surface across Linux, macOS, Windows, and
WebAssembly. Tests could be written beside each ported rule and executed
immediately, which was particularly effective for AI-assisted work.

## The compiler pipeline

The first major task was to identify the effective C++ production path. The
port then preserved that path as an explicit sequence:

```text
Faust source
  -> parser
  -> Box tree
  -> evaluation
  -> Box-to-Signal propagation
  -> signal preparation and normalization
  -> signal type and interval analysis
  -> transformation and scheduling
  -> FIR
  -> source backend, interpreter, JIT, or WebAssembly
```

Each boundary has a distinct responsibility. Parsing constructs Faust's Box
language. Evaluation resolves definitions, applications, environments,
iteration, pattern rules, and recursion. Propagation converts evaluated boxes
into signal expressions. Preparation canonicalizes recursion and inserts
promotions. Typing and interval analysis establish numeric nature,
variability, computability, vectorability, and bounds. Transformations decide
placement, scheduling, delays, clock domains, differentiation, and vector
plans. FIR describes the complete executable module. Backends consume FIR
instead of independently rediscovering compiler semantics.

This separation made progressive implementation possible. A failure could be
located at the earliest boundary where Rust diverged from C++, rather than
being diagnosed from generated source alone.

## Four useful groups of crates

The workspace contains many crates because the C++ compiler was decomposed
along dependency and ownership boundaries. The following four groups provide a
more useful map than an alphabetical crate list; they are conceptual groups,
not four monolithic layers.

1. **Source language and shared trees.** `tlib` owns the hash-consed
   `TreeArena`; `boxes`, `parser`, and `eval` implement the Box language;
   `propagate` and `signals` form the signal graph. These crates contain the
   language-facing semantics and the canonical builder/matcher APIs.

2. **Analysis, transformation, and executable IR.** `interval`, `sigtype`,
   `normalize`, `transform`, `fir`, and `ui` own typing, bounds,
   canonicalization, scheduling, delay and recursion planning, clock domains,
   automatic differentiation, vector planning, FIR construction, and FIR
   verification. This is where semantic decisions become explicit data rather
   than backend conventions.

3. **Orchestration, rendering, and execution.** `compiler` drives the complete
   pipeline; `codegen` contains the source, interpreter, JIT-related, and
   WebAssembly lowering paths; `draw` translates evaluated boxes into Faust
   block-diagram layouts and SVG. These crates turn verified compiler
   representations into user-visible artifacts or executable DSPs.

4. **Interop and development tools.** `tree-ffi`, `box-ffi`, `signal-ffi`,
   `interp-ffi`, `cranelift-ffi`, `wasm-ffi`, and `faust-ffi` expose
   Faust-shaped C and C++ surfaces. `foreign-call` supports external symbols,
   while `impulse-runner` and `xtask` provide validation and maintenance
   commands. These boundaries keep raw pointers, C strings, dynamic-library
   exports, and test orchestration out of the semantic core.

Some crate boundaries can still be clarified or consolidated, but the central
pipeline is not split arbitrarily: lower crates cannot depend on orchestration
or target-specific APIs, and public FFI ownership remains isolated from safe
Rust representations.

## Five decisions that shaped the port

### 1. Preserve semantics through stages, not lines

The project began with an inventory of roughly 159,000 lines of C++ and
headers, followed by a dependency-ordered plan. Historical directory layout
was not treated as architecture: pattern matching was placed with evaluation,
extended mathematical nodes with signals, and parallelization with
transformation. Java and legacy OCPP were excluded explicitly. Before deep
implementation, the parser, tree arena, real signal-to-FIR path, global-state
coupling, and public API lifecycles were studied or prototyped.

This prevented a common porting failure: faithfully translating an inactive,
legacy, or target-specific path while missing the behavior users actually
exercise.

### 2. Use one hash-consed substrate with typed vocabularies

C++ Faust relies heavily on shared immutable trees and pointer identity. Rust
replaced this with one `TreeArena`, dense copyable `TreeId` handles, structural
hash-consing, interned symbols and tags, compact child storage, and pass-owned
property maps. Boxes, Signals, and FIR do not use competing tree
implementations: `BoxId`, `SigId`, and `FirId` are domain aliases over the
same arena model.

Each language is exposed through a canonical builder and an enum-based matcher:
`BoxBuilder`/`BoxMatch`, `SigBuilder`/`SigMatch`, and
`FirBuilder`/`FirMatch`. This combines compact structural sharing with
exhaustive Rust matching. The arena was benchmarked and tuned before the rest
of the compiler depended on it; safety was not accepted as a reason to ignore
a central performance risk.

### 3. Make FIR the shared trust boundary

FIR was introduced early so backend development did not have to wait for the
whole front end. An AI agent first generated, at development time, a FIR module
for a sine oscillator built from a phasor, UI controls, state, and a sample
loop. That hand-built module brought up the C++ and C emitters while upstream
stages were incomplete. The first real signal-to-FIR lane then connected Faust
source to those backends.

FIR progressively became more than a syntax-neutral instruction list. It owns
types, lifecycle sections, state accesses, functions, loops, and the explicit
sample loop. A module verifier checks scope, initialization, access classes,
types, calls, symbols, and the DSP API contract before a backend runs.
Consequently, C, C++, the interpreter, Cranelift, WebAssembly, Julia,
AssemblyScript, and Rust can share one execution model rather than duplicating
semantic decisions.

### 4. Establish a simple correct model before optimizing it

The first complete chain computed almost everything inside the sample loop.
That code was valid but inefficient, and deliberately so: it isolated semantic
errors before adding placement, lifetime, and sharing decisions.

The same method was applied to time and state. Delays and recursive history
first converged on one power-of-two circular-buffer model driven by a
persistent `fIOTA` cursor. Once this was correct, analysis and emission were
separated. A `DelayManager` introduced shift, circular, and exact-size
strategies; recursion acquired its own ownership model; both could share
storage when analysis proved that the histories were equivalent.

Full signal typing, normalization, and interval analysis then supplied generic
facts needed by optimization. Variability placed constants in
`instanceConstants`, block-rate values in the compute preamble, and
sample-rate values in the loop. FIR common-subexpression elimination
materialized repeated values once per appropriate execution region. These
optimizations benefited every backend because they lived before emission.

### 5. Treat executable evidence and refusal as product features

The project rejected case-specific “patches” that merely made one DSP pass.
An interval failure had to be corrected in interval semantics; a malformed
signal spelling had to be normalized before FIR; a lifecycle problem had to be
fixed in the shared contract. Unsupported behavior produced a typed diagnostic
or an explicit scalar fallback instead of plausible output.

This rule was essential for AI-assisted development. Agents provided broad
code search, repetitive implementation, tests, and cross-layer repair speed.
The maintainer remained responsible for semantic references, architecture,
compatibility decisions, and rejecting locally attractive but non-general
fixes. AI accelerated the work, but executable oracles and human review
determined what became the compiler contract.

## FIR validation and runtime feedback

### The FIR module verifier

Once FIR had more than one producer and more than one consumer, successful
backend compilation was no longer a sufficient validity check. A malformed
module might happen to compile in C++, fail only in one JIT path, or acquire a
different meaning in two emitters. On February 23, the project introduced a
module-wide FIR verifier so that all consumers could rely on one checked
contract.

The verifier checks the structure and relationships of a complete module,
including:

- required module and DSP lifecycle sections;
- duplicate declarations, missing symbols, and function signatures;
- struct, global, local, argument, loop, and table access classes;
- lexical scope, declaration order, and initialization;
- expression, assignment, return, and numeric-conversion types;
- function-call existence, arity, and argument/result compatibility;
- the canonical host-facing DSP API expected by backends.

This differs from relying on target compilers as late validators. An invalid
FIR producer or transform is rejected before target syntax obscures the cause,
and the same error is seen whether the next consumer is C++, the interpreter,
Cranelift, or WebAssembly. The verifier also made hand-built FIR fixtures safe
enough to remain useful during backend bring-up.

On February 24, ownership of the sample loop moved from the early C and C++
emitters into FIR itself. Lifecycle sections and execution order therefore
became part of the verified module rather than conventions reconstructed by
each backend. This was the point at which FIR became a genuine trust boundary:
producers own semantic correctness, the verifier enforces structural and type
invariants, and backends lower an already explicit execution model.

See the [FIR module verifier plan](../porting/fir-module-verifier-plan-en.md).

### The interpreter: runtime feedback without a C++ toolchain

The interpreter was planned and implemented on February 21–22, only a week
after the project began. It introduced typed FBC bytecode instructions, a
FIR-to-FBC compiler, a bytecode optimizer, an executor, factories,
serialization, DSP instances, UI construction, metadata, and lifecycle
support.

Its importance was greater than the addition of one backend. Before it,
runtime validation meant emitting C or C++, invoking an external compiler,
linking a host harness, and then executing the result. The interpreter made the
same source-to-FIR pipeline directly executable inside Rust. A small DSP could
be compiled, initialized, supplied with audio and controls, and inspected
sample by sample in one test process.

That short feedback path exposed temporal bugs that generated-source review
rarely reveals clearly: delay read/write order, recursive-state advancement,
table effects, UI defaults, block-boundary behavior, lifecycle resets, and
optimizer-induced drift. Runtime traces could be compared with C++ output or
with another backend at the first differing sample and state transition.

The runtime retained two execution surfaces: a fast path for normal use and a
checked path that reports malformed bytecode or test failures explicitly.
Representative tests also compare unoptimized and optimized FBC execution, so
the bytecode optimizer is not trusted merely because it produces valid
instructions. The interpreter consequently became both a user-facing runtime
and the quickest executable semantic oracle produced by the Rust port itself.

## A more precise backend map

FIR allowed different kinds of backend to share compiler semantics without
pretending that they had identical maturity or deployment models.

| Backend or artifact | Role and output | Historical status and evidence |
|---|---|---|
| **C and C++** | Portable source emitters with Faust DSP lifecycle, UI, metadata, precision, architecture-file wrapping, and explicit FIR loops | First complete targets and the most mature scalar paths; generated code is compiled and exercised by the full four-pass impulse protocol |
| **Interpreter** | FIR lowered to serializable FBC bytecode and executed by the in-process Rust runtime | Brought up in the first week; used for fast traces, optimization parity, factories, and native C/C++ embedding |
| **Cranelift** | FIR lowered to native JIT code with retained module and DSP runtime state | Proved that FIR was not tied to text generation; executable and cross-checked with the interpreter, but still experimental and without a final serialization format |
| **WebAssembly / WAT** | Direct binary WASM or textual WAT, with linear-memory layout, lifecycle exports, imported math/foreign functions, and companion JSON | Integrated with `faustwasm` mono/poly browser paths; functional without claiming complete byte-for-byte parity with the C++ WASM backend |
| **AssemblyScript** | Typed AssemblyScript source compiled to WASM by the external `asc` toolchain | Added as a readable source-to-WASM route and executed under Node in the shared impulse harness |
| **Julia** | Julia source with a mutable DSP structure, lifecycle, UI, metadata, precision, casts, and `compute!` | Functional first slice; useful but less mature than C/C++ and dependent on the host Julia Faust runtime contract |
| **Rust** | Standalone Rust source implementing the established Faust Rust architecture contract (`FaustDsp`, host numeric types, UI and metadata traits) | Added in July and validated through a 92/92 impulse gate on the imported C++ population |
| **Mojo** | Mojo source with the `FaustDsp` struct contract of the open upstream pull request grame-cncm/faust#1293, plus a Rust-only batch form that lays out N voices as a structure of arrays for GPU runs | Added in November; 132/132 in the opt-in impulse lanes (outside CI); the GPU measurements are summarised in `porting/faust-gpu-audio-synthesis-2026-11-07-en.md` |
| **FIR text** | Canonical diagnostic and inspection view of the verified module | Intended for inspection, fixtures, and compiler development rather than deployment as a host runtime |

Strict `-json` and SVG diagrams are auxiliary products, not execution
backends. JSON describes the compiled DSP and its UI, while `draw` translates
the evaluated Box tree into a laid-out diagram before Signal lowering erases
its block algebra. Similarly, the presence of a backend directory does not by
itself mean that the corresponding language is an operational compiler target:
the project distinguishes scaffolding, generated syntax, executable output,
corpus validation, and parity.

## Public C and C++ libraries

### Interpreter and Cranelift factories

The Rust compiler became embeddable almost as soon as it became executable.
On February 22, `interp-ffi` exposed a Faust-shaped C ABI and thin C++ wrapper
around the interpreter. Cranelift followed from February 25 onward. Both
surfaces use the familiar Faust factory/instance organization:

```text
Faust file or source string
  -> compiler pipeline
  -> backend factory
  -> one or more DSP instances
  -> init / UI / metadata / compute
```

Opaque handles prevent Rust layouts from becoming ABI, while explicit create
and delete functions preserve ownership on the C side. The lifetime rule is
important: a factory owns compiled backend material and must outlive every DSP
instance created from it. Factory caches avoid recompiling identical sources.
The APIs translate option arrays, errors, audio-buffer pointer arrays, C
strings, UI callbacks, and metadata callbacks at the boundary rather than
letting unsafe representations enter the compiler core.

Interpreter factories can be created from Faust files or strings, serialized
to and restored from FBC, cached, queried, and used to create executable
instances. Cranelift factories follow the same overall lifecycle and create
native JIT-backed instances from the compiler's FIR. Cranelift-specific
serialization remained a documented scaffold rather than being presented as
a final equivalent of LLVM bitcode.

In May, `generateAuxFiles` also exposed compiler-produced JSON and SVG
artifacts through the interpreter and Cranelift C/C++ surfaces. Native hosts
could therefore use more than the runtime `compute` path: the same library
could compile sources, inspect metadata and UI, serialize interpreter code,
and request auxiliary compiler products.

The `faust-ffi` distribution crate links the interpreter, Cranelift, Box, and
Signal export crates into one static library and one platform dynamic library.
The artifacts are named `libfaust-rs.a` and `libfaust-rs.dylib`/`.so`, or
`faust-rs.dll`, so they can coexist with official C++ `libfaust`. Maintained C
and C++ headers provide familiar entry points without requiring callers to
understand Cargo crates or Rust ownership. The distribution initially used the
compatibility name `libfaust`; it was renamed `libfaust-rs` on July 23 so the
Rust and official C++ implementations could be installed and linked side by
side.

See the [interpreter FFI plan](../porting/faust-rust-ffi-interp-en.md) and
[Cranelift FFI parity matrix](../porting/cranelift-dsp-ffi-parity-matrix-en.md).

### Exporting the Box and Signal APIs

Runtime factories expose compilation and execution, but libfaust has another
essential role: programs such as visual editors, language bindings, and DSP
construction tools need to build and inspect Faust's intermediate languages
directly. The first Box C/C++ layer appeared on February 27. Beginning on June
9, it was expanded into a systematic Box and Signal API parity effort against
the official `libfaust-box-*` and `libfaust-signal-*` headers.

The external surface deliberately mirrors established libfaust conventions
while the internal Rust surface remains builder- and matcher-based. It
includes:

- creation and destruction of shared tree contexts;
- opaque Box and Signal handles with common arena provenance;
- Box and Signal constructors, including composition, recursion, tables,
  waveforms, soundfiles, UI, and mathematical primitives;
- structural predicates and decomposition functions;
- Box arity queries and Box-to-Signal propagation;
- Signal normal form, printing, type queries, and source generation;
- null-terminated result arrays and explicit `freeCMemory` ownership;
- thin C++ overloads layered on the stable C ABI.

The shared `tree-ffi` layer is central to this design. Box and Signal handles
must refer to one compatible arena and allocation context so that conversion
does not copy arbitrary foreign trees or mix unrelated identities. `box-ffi`
owns the Box surface, `signal-ffi` owns the Signal surface, and the top-level
distribution exports both together with the executable runtimes.

Source generation through these compatibility APIs was kept narrower than the
CLI: the validated Box/Signal entry points supported C, C++, FIR, and
interpreter output, while other languages remained available through the
compiler facade. Rust extensions such as FAD and RAD were exported explicitly
instead of being mislabeled as C++ parity.

This work was validated as an ABI product, not only as Rust code. Symbol
matrices were generated from the reference headers; C11 and C++17 clients
compiled against the maintained headers; header inclusion order was tested;
and `xtask libfaust-export-check` compared declarations with the actual
dynamic-library exports. The June checkpoint found all 269 declared Box and
Signal C symbols in the produced library.

The result is an important part of the port's usability: C and C++ hosts can
compile Faust text into interpreter or Cranelift factories, execute DSP
instances, or construct and transform Box and Signal graphs directly, all
through one Faust-shaped native distribution.

See the [Box/Signal API parity plan](../porting/libfaust-box-signal-api-parity-plan-2026-06-09-en.md).

## Expansion beyond the first scalar compiler

WebAssembly work in late March had two roles. The compiler emitted DSP WASM
with a defined memory layout, lifecycle exports, math imports, and companion
JSON; the Rust compiler itself was also built as WebAssembly for browser use
through `faustwasm`. This connected the port to Faust's Web Audio ecosystem
instead of creating a separate Rust-only runtime.

The typed description developed for WASM also supported the standalone,
backend-neutral `-json` contract. In May, the `draw` crate added `-svg` by
translating evaluated Box trees into laid-out, linked Faust block diagrams;
both artifact families were also exposed to native and browser embedding APIs.

New compiler features were added through the same staged discipline.
Forward-mode automatic differentiation became `fad(expr, seeds)` with explicit
lanes and augmented recursive state. Reverse mode became `rad(expr, seeds)`;
general stateful differentiation uses an explicit block-local reverse pass and
truncated-backpropagation-through-time contract rather than an overclaimed
infinite-horizon gradient.

The `ondemand`, `upsampling`, and `downsampling` primitives introduced nested
clock domains whose local state advances only when the domain fires. FAD can
cross supported boundaries by augmenting one clocked block with primal and
tangent lanes, avoiding duplicate state advancement. Unsupported RAD
crossings remain loud errors.

Vector compilation preserves `-vec`, `-vs`, and `-lv`, but moves planning
ahead of FIR. A typed `VectorPlan` records loop ownership, dependencies,
effects, epochs, and transports; an independent checker validates finite
structural invariants before lowering. Uncertified cases fall back explicitly
to scalar code. A Lean model explores selected scheduling properties, but it
is genuinely experimental: it is not a proof of the Rust compiler, generated
DSP semantics, or scalar/vector equivalence.

## What supports confidence

“The tests pass” would be too weak a summary of the evidence. The validation
system combines independent layers:

- unit and crate-level integration tests added with each ported rule;
- three-OS workspace formatting, linting, build, and test gates;
- Rust golden outputs and differential outputs from a pinned C++ compiler;
- a module-wide FIR verifier before backend consumption;
- interpreter and JIT runtime comparisons, including optimized versus
  unoptimized execution where relevant;
- generated-source compilation and host lifecycle tests;
- C and C++ header, ABI-surface, and export checks;
- the C++ `impulse-tests` oracle, executing 60,000 frames in scalar,
  randomly-blocked, four-voice, and one-voice passes;
- mutation tests for independent schedule and vector-plan checkers;
- checked-in capability and vector-admission reports that distinguish actual
  vector plans from scalar fallback.

These gates found real shared bugs in delay ordering, lifecycle,
`instanceClear`, precision, reverse loops, vector storage, and backend casts.
The strongest claim supported by them is operational comparability on the
tracked envelope, not proof of the full Faust language.

## Current limits

At the July 23, 2026 cutoff of the detailed history, scalar C and C++ were the
most mature production paths. Interpreter, Cranelift, WebAssembly,
AssemblyScript, Julia, and Rust were functional but had different validation
depths. WebAssembly layout was integrated with `faustwasm`, but complete
byte-level C++ backend parity was not claimed. Julia remained an initial
high-level backend slice. Cranelift was an experimental Rust-native JIT, not a
delivered LLVM replacement.

Vector mode was checked and executable where admitted, but did not vectorize
the entire corpus and did not yet have a complete profitability model. The
regenerated July baseline admitted 97 of 133 DSPs in all 16 tested vector
modes; other programs had explicit fallbacks or an error. Lean remained an
assurance experiment with no end-to-end refinement proof.

Remote URL imports, parts of the embedded compiler helper surface, RAD across
clock domains, full long-tail API parity, and behavior outside the tracked
corpus remained incomplete. Java and legacy `-lang ocpp` were outside scope.
External control-rate separation (`-ec`) and one-sample execution (`-os`) had
a porting plan but were not yet implemented at that cutoff.

These limits matter because the project distinguishes four different claims:
implemented, executable, validated on a corpus, and parity-equivalent. They are
not interchangeable.

## Condensed timeline

| Period | Milestone |
|---|---|
| February 14–16 | C++ inventory, staged plan, `TreeArena`, parser, Boxes, evaluation, propagation, and first Signal path |
| February 17–18 | Canonical FIR, AI-built phasor fixture, signal-to-FIR fast lane, first complete C/C++ chain |
| February 21–27 | Interpreter, FIR verifier, FIR-owned loop, Cranelift, runtime FFI, and differential execution |
| March–early April | Full signal types, intervals, normalization, `fIOTA`, WebAssembly, strict JSON, placement, CSE, and refined delays |
| Mid-April | First operational scalar milestone, about two months after the project began |
| April–May | FAD, RAD, SVG, Julia, and expanded delay/recursion models |
| June | Box/Signal APIs, AssemblyScript, systematic multi-backend impulse tests, and clock/vector design |
| July | Clock domains, FAD composition, checked vector plans, experimental Lean model, Rust backend, and expanded coverage reports |

## Central lesson

The port advanced quickly because implementation and feedback were both made
incremental. Typed IR boundaries let unfinished stages develop independently;
the interpreter and hand-built FIR fixtures shortened runtime feedback; Rust
made local tests inexpensive; C++ differential and impulse oracles prevented a
locally convincing implementation from being mistaken for parity.

The AI experiment was positive but conditional. Agents materially accelerated
codebase exploration, repetitive Rust implementation, test production,
documentation, and cross-layer diagnosis. They did not replace DSP expertise,
semantic authority, compatibility decisions, or disciplined review. The
durable result came from combining agent velocity with one explicit pipeline,
generic fixes, fail-closed policies, and progressively stronger executable
evidence.
