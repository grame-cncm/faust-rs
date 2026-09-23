# From Faust C++ to `faust-rs`: a porting history

> This is the complete technical history. Readers looking for a shorter
> introduction should start with the
> [concise porting history](faust-cpp-to-rust-port-history-overview-en.md).

## Introduction

The `faust-rs` project began in February 2026 with an ambitious objective:
rebuild the Faust compiler in Rust while preserving the semantics of the
production C++ compiler.

It also had a second, deliberately experimental objective: evaluate, on a
concrete project of significant size, whether AI agents could contribute
effectively to a serious compiler port. The test was broader than asking an AI
to translate isolated functions. Agents had to explore a large and historically
layered codebase, reconstruct its real execution paths, propose staged plans,
write and review Rust across many crates, diagnose cross-layer regressions, and
work against executable C++ oracles.

This made the project an experiment in AI-assisted software engineering as much
as a language migration. The scope included substantial simplification,
cleanup, and re-architecture: decomposing global state, replacing inheritance
and visitor families with typed Rust representations, making implicit
lifecycles explicit, separating analysis from emission, and introducing
verifiable IR boundaries. It also included adding capabilities that were not
present in the pinned C++ reference, such as FAD/RAD differentiation, new
backends, checked vector planning, and new combinations of clock-domain and
learning features. The practical question was whether agent velocity could
remain useful when the work demanded architectural judgment, long-range
consistency, and generic fixes rather than local code generation.

This was never intended to be a line-by-line translation. Faust is a mature
compiler whose source language, tree representations, evaluator, signal
algebra, optimizations, runtime model, and many backends have evolved together
for more than two decades. A successful port had to preserve those
relationships while replacing C++ implementation mechanisms—global state,
inheritance, visitors, pointer-owned trees, and partial late checks—with Rust
ownership, typed enums, explicit contexts, arenas, and verifiable intermediate
contracts.

The first two months established the central result: an operational scalar
compiler pipeline, comparable to the official C++ scalar path on the tracked
portable corpus. The path to that result was deliberately incremental. The
team first mapped the entire C++ codebase, then built one executable compiler
spine from the tree library upward. Correctness came before optimization, and
each new corpus failure was treated as evidence about a missing general rule,
not as an invitation to add a local patch.

This account is reconstructed from the initial plans, the daily porting
journal, the Git history, and later project assessments. It describes both what
happened and why the sequence worked.

## Why Rust was worth the port

The choice of Rust was not only motivated by replacing an old codebase with a
newer language. Its language model, compiler, and standard toolchain addressed
several concrete difficulties of a large compiler project.

### Language-level guarantees as architecture feedback

Rust removes broad classes of failures without requiring a garbage collector:
use-after-free, dangling references, accidental double ownership, unchecked
null values, and unsynchronized shared mutation cannot be expressed through
ordinary safe code. This is directly relevant to Faust, whose C++
implementation historically combines arena-owned trees, shared node identity,
global compiler state, visitor objects, and lifecycle-sensitive factories.

The ownership and borrowing rules did more than prevent memory bugs. They acted
as architectural feedback. When a direct translation required several passes
to mutate the same global tree or context simultaneously, the borrow checker
made that coupling visible. The usual resolution was to identify the real
owner, give each pass an explicit context, externalize analysis properties, or
split construction from inspection. In that sense, compilation failures often
pointed toward the cleanup and re-architecture the port was meant to achieve.

Rust's algebraic data types were equally important:

- `enum` variants replaced open-ended class hierarchies, RTTI, and
  `dynamic_cast`;
- exhaustive `match` expressions made missing Box, Signal, FIR, type, or
  opcode cases visible when a representation evolved;
- `Option` and `Result` replaced many sentinel, null-pointer, and implicit
  exception contracts;
- newtypes such as `BoxId`, `SigId`, and `FirId` prevented accidental mixing of
  handles that were all pointer-shaped in C++;
- traits expressed shared behavior without forcing target-specific code into
  one inheritance tree.

These properties did not prove the Faust semantics correct, but they moved many
representation and lifecycle errors from runtime into the compiler's own type
checking.

### The compiler and toolchain as one development surface

Cargo provided a uniform workspace for the port's many crates. Dependency
resolution, conditional features, library/binary targets, examples,
benchmarks, documentation, and tests all use the same metadata and commands.
The normal quality loop became:

```text
cargo fmt
  -> cargo clippy
  -> cargo test
  -> cargo build
```

`rustfmt` removed formatting debates, Clippy turned common suspicious patterns
into enforceable diagnostics, and rustdoc kept public types and provenance
close to the code. Cargo's incremental compilation and package filters also
made it practical to validate one crate while developing, then run the entire
workspace gate before integration.

The same source workspace builds on the three principal desktop operating
systems. CI has run the compiler and its tests on Linux, macOS, and Windows,
forcing path handling, process invocation, dynamic-library naming, and test
fixtures to remain portable. Target-specific backends can still have platform
constraints—JITs and native linkers do not become universal merely because
their host is Rust—but the compiler's common build and test surface no longer
depends on maintaining separate CMake paths for each OS.

Rust also made WebAssembly a normal compilation target:

```text
cargo build --target wasm32-unknown-unknown
```

This was crucial for `faust-rs`: not only can it emit DSP WebAssembly, but the
compiler itself can be packaged as a raw WASM module and executed in
`faustwasm`. The same ownership-safe Rust code therefore serves native CLI,
library, and browser compilation roles, subject to keeping the relevant crates
free of unavailable host services.

### Unit tests written “on the fly”

The most immediate productivity gain was the ability to add unit tests beside
the code being ported. A Rust source file can contain a private
`#[cfg(test)]` module with direct access to its internal builders and helpers;
there is no need to create a separate test executable, expose private symbols,
or update a second build system. `cargo test` discovers and builds these tests
automatically.

That changed the granularity of the port. An agent implementing one parser
production, tree matcher, interval rule, delay strategy, FIR verifier check, or
backend instruction could add focused tests in the same change and execute
only that crate or test name immediately. When a corpus DSP exposed a bug, the
generic correction could first be locked by a tiny unit test at the owning
layer, then by an integration or differential case at the compiler boundary.

The resulting validation ladder grew naturally:

```text
module unit test
  -> crate integration test
  -> workspace test
  -> Rust golden output
  -> differential test against C++
  -> executable backend / impulse test
```

This “test while porting” workflow was particularly well suited to AI-assisted
development. Agents received rapid, local feedback rather than waiting for a
large compiler to be complete, while the broader gates prevented a locally
plausible implementation from being mistaken for semantic parity. Rust did not
remove the need for C++ oracles and DSP expertise; it made it inexpensive to
turn each discovered invariant into a permanent executable check.

These motivations were present in the
[initial porting plan](../porting/faust-rust-porting-plan-en.md), before the
implementation results made their practical value visible.

## 1. Before writing Rust: making the C++ compiler legible

The project started with an inventory rather than an implementation. The
reference branch, `master-dev-ocpp-od-fir-2-FIR19` at commit `8eebea429`, was
measured at approximately 159,000 lines of C++ and headers. Roughly 300 source
files were classified by role and dependency.

The analysis reconstructed the effective compilation path:

```text
Faust source
  -> parser
  -> box diagrams
  -> evaluation
  -> box-to-signal propagation
  -> signal normalization
  -> signal type and interval analysis
  -> scheduling and transformation
  -> FIR
  -> backend
```

It also separated conceptual architecture from historical directory layout.
For example:

- `patternmatcher` belonged with evaluation;
- extended mathematical signal nodes belonged with signals;
- `parallelize` belonged with transformation and scheduling;
- `compiler` would remain the orchestration layer;
- FIR and backends would be independent consumers of a canonical IR;
- Java and the legacy OCPP mode were explicitly excluded.

This initial decomposition became a dependency-ordered implementation plan:
foundations and TLIB first, then boxes and parsing, evaluation and propagation,
normalization and types, FIR, backends, and finally integration and public APIs.
The plan also identified the highest risks before they became blockers:
parser compatibility, `TreeArena` performance, hidden `gGlobal` coupling,
selection of the correct signal-to-FIR path, backend lifecycle contracts, and
the size of the public `libfaust` surface.

The project did not blindly trust the plan. Phase 0 required prototypes and
measured gates. The `TreeArena` design was benchmarked against the C++ tree
implementation before higher compiler layers committed to it. Parser work
started with differential fixtures against C++ rather than with the assumption
that accepting a grammar was enough.

The original analysis and phase structure are recorded in:

- [the overall porting plan](../porting/faust-rust-porting-plan-en.md);
- [the initial assessment](../porting/faust-rust-bilan-global-en.md);
- [the Phase 0 validation gate](../porting/phases/phase-0-validation-en.md).

## 2. TLIB first: rebuilding the substrate

Faust boxes and signals are trees, so the C++ `tlib` was the natural first
implementation target. It was also one of the most consequential architectural
choices.

The C++ compiler relies on pointer identity, hash-consing, symbols, cons lists,
properties attached to tree nodes, and allocation machinery shared across the
compiler. The Rust design replaced that with:

- a `TreeArena` owning all nodes;
- compact, copyable `TreeId` handles;
- hash-consing so structurally identical nodes share an identity;
- interned symbols;
- tree-encoded lists;
- typed property maps external to the nodes.

External property maps were particularly important. They replaced mutable
properties embedded in shared C++ trees with pass-owned analysis state. This
made ownership explicit and allowed independent analyses to borrow the same
arena safely.

The team did not accept a slow abstraction merely because it was safe. On
February 15, the arena and property paths went through repeated benchmark and
layout iterations: specialized interning for common arities, compact child
storage, faster hash maps, interned node tags, and pre-allocation. Only after
the performance gate closed did the rest of the compiler build on the arena.

This early discipline mattered later. The same arena and structural sharing
model could represent boxes, signals, and FIR instead of creating three
unrelated object systems.

## 3. Rust enums as the compiler's vocabulary

The port used Rust's type system to replace several C++ class hierarchies and
visitor families.

Boxes, signals, and FIR all follow the same canonical pattern:

```text
typed ID stored in an arena
  + one builder API
  + one enum-based matcher
  + exhaustive Rust match expressions
```

The concrete APIs became:

- `BoxBuilder` with `BoxMatch` and `match_box`;
- `SigBuilder` with `SigMatch` and `match_sig`;
- `FirBuilder` with `FirMatch` and `match_fir`.

These match enums are typed views over hash-consed arena nodes. They preserve
the compact shared-tree representation while giving each compiler pass an
algebraic data type to inspect. Adding a node family therefore has an explicit
construction path and an explicit matching path. Exhaustive `match` statements
make missing cases visible during development instead of hiding them behind
RTTI, unchecked casts, or a default visitor method.

FIR also gained an explicit `FirType` enum and typed result information on value
nodes. C++ visitors sometimes reconstruct types late or consult global
compiler state. In Rust, FIR values carry the type information needed by
checkers and backends.

This was not just a stylistic modernization. The common builder/matcher
contract made it possible for AI-assisted implementation to add node families
quickly without proliferating incompatible helper APIs. It also made code
review sharper: every new representation choice could be checked at the enum,
builder, matcher, and test boundaries.

The FIR contract is described in
[FIR Architecture Contract](../porting/faust-rust-fir-architecture-en.md).

## 4. The first executable front end

Once TLIB and boxes were usable, the front end advanced quickly:

- February 15: parser prototype, lexer and grammar slices, box construction,
  import handling, diagnostics, and C++ differential fixtures;
- February 16: canonical signal builders, evaluation, environments,
  applications, iteration, pattern matching, box-to-signal propagation, and
  recursive propagation;
- February 16: the first production
  `parse -> eval -> propagate -> signals` compiler path and `dump-sig` mode.

The order was important. Evaluation was not reduced to syntax lowering. Faust
evaluation includes environments, closures, pattern rules, partial
application, recursive definitions, and compile-time expansion. Propagation
then converts evaluated boxes into a graph of typed signal operations. Treating
these as separate stages kept source-language semantics out of backend code.

Differential validation was present from the beginning. Parser results and
front-end acceptance were compared with the C++ compiler on a growing set of
small DSPs. Failures could be localized to parsing, evaluation, or propagation
before FIR existed.

### From isolated sources to real Faust libraries

Supporting grammar productions was only part of making the front end useful.
Real Faust programs are networks of source files: local imports, nested
imports, libraries selected through `-I`, inline environments, and metadata
must retain their origin as they are expanded.

On February 15, `SourceReader` acquired import expansion, search paths, and
cycle detection. File compilation used the input file's parent directory by
default, then explicit import directories in deterministic order. Import-heavy
fixtures checked nested local resolution, missing paths, and cycles against
C++. By February 28, parser results also reported every source file actually
used and preserved imported-file origins in diagnostics.

The implementation kept URL fetching separate from local resolution. Remote
HTTP/HTTPS imports depended on the C++ `sourcefetcher` subsystem and were
explicitly frozen as unsupported instead of making parser behavior depend on
network access. At the end of March, import expansion was further aligned with
the evaluated tree model: `importFile` nodes inside structural and inline
environments were expanded with definition-local duplicate suppression, while
top-level and transitive source provenance remained visible.

This work later supported two different library environments:

- native compilation resolves files through local and configured Faust library
  paths;
- the WebAssembly compiler module receives read-only embedded standard
  libraries plus caller-supplied virtual sources.

Keeping virtual sources behind the same source-reading contract allowed browser
compilation, SVG generation, and auxiliary-file services to use libraries
without inventing an Emscripten filesystem. Tests that exercise library-style
behavior remain self-contained: they use compact inline definitions rather
than depending on a developer's local Faust installation.

### Architecture files and `enrobage`

A generated DSP class alone is not a usable Faust application. Faust
architecture files supply the host runtime—audio driver, UI glue, process
entry point, plugin wrapper—and contain markers where the compiler injects the
generated class. Preserving this mechanism was essential for existing
`faust2*` workflows.

The C++ `enrobage` subsystem was studied and ported on February 19. The work
covered:

- `-a` architecture-file selection and repeatable `-A` search directories;
- deterministic direct-path and search-path lookup;
- optional `-i` inline inclusion of referenced Faust architecture headers;
- license-header preservation/removal rules;
- `<<includeIntrinsic>>` and `<<includeclass>>` marker handling;
- `mydsp`/`dsp` class-name replacement at the same token boundaries as C++;
- include de-duplication and recoverable missing-include diagnostics;
- portable `PathBuf`-based output naming and directory handling.

The port separated pure path helpers, search/open behavior, stream copying, and
final compiler integration, with golden fixtures and a differential report for
each layer. C++ output was wired first, followed immediately by C. FIR output
rejects architecture wrapping because FIR is an intermediate representation,
not source code to embed in a host template.

This early work established a compatibility principle applied to every later
backend: generated code must expose the public DSP lifecycle, buffer, precision,
UI, class-name, and metadata contract expected by its existing architecture
ecosystem. Backend syntax can be new; the host-facing contract cannot be
silently redesigned.

See [enrobage porting plan](../porting/phases/phase-9-enrobage-porting-plan-en.md)
and [enrobage differential report](../porting/phases/phase-9-enrobage-diff-report-en.md).

## 5. FIR as the acceleration point

FIR changed the pace of the project.

On February 17, `faust-rs` established the canonical FIR builder/matcher
architecture, explicit value types, arena-backed structural sharing, and the
first module-oriented C++ emitter. FIR became the stable boundary between
compiler semantics and target-language syntax.

The first substantial backend experiment did not wait for the complete
signal-to-FIR compiler. An AI agent generated FIR directly, at development
time, for a phasor-driven sine oscillator:

```faust
freq = hslider("freq", 440, 20, 3000, 1);
gain = hslider("gain", 0.2, 0, 1, 0.001);
phase = +(freq / 48000.0) ~ _;
process = gain * sin(2.0 * ma.PI * phase);
```

The resulting `build_sine_phasor_test_module()` fixture contained UI controls,
a phase accumulator, state updates, a sample loop, and an output store. The
same FIR module was fed to backend examples and tests. This isolated the C++
emitter from the unfinished front end and answered a decisive question early:
could the Rust FIR express a real stateful DSP and could the backend turn it
into valid Faust-style C++?

The answer was yes. The fixture also established an enduring testing pattern:
backend bring-up can use hand-built, backend-independent FIR modules, while
end-to-end tests separately validate the real source-to-FIR producer.

The relevant Git milestone is `f844d7d0`, “Align C++ backend with dsp.h contract
and add FIR sine/phasor fixture”.

## 6. A complete compiler chain in four days

The next step was to replace the hand-built oscillator FIR with FIR produced
from real Faust signals.

On February 18, a signal-to-FIR “fast lane” was wired through the compiler. It
started with a deliberately limited but real subset: arithmetic, state,
controls, delays, tables, UI, and module lifecycle sections. C and C++ could
then consume the same FIR.

Four days after the repository started, the project had a complete executable
spine:

```text
Faust source
  -> parser
  -> boxes
  -> evaluation
  -> propagation
  -> signals
  -> FIR
  -> C or C++
```

The generated code was valid for the supported subset. It was not yet good
code.

The initial lowering placed essentially all executable signal expressions in
the sample section. Constants, slider-derived values, and repeated
subexpressions could therefore be recomputed inside the audio loop. This was a
conscious staging decision. A simple, uniform “execute it per sample” model
made semantic debugging possible before adding execution-rate placement,
lifetime analysis, or sharing preservation.

That distinction—valid first, optimized later—was central to the project's
speed. It avoided debugging type inference, scheduling, lifetime, delay
geometry, and backend syntax simultaneously.

## 7. Making generated FIR trustworthy

As soon as multiple producers and backends existed, “the code compiles” was no
longer a sufficient invariant.

The project progressively added several validation layers:

### Golden and differential workflows

The repository had Rust and C++ golden workflows from its first day. Corpus
cases recorded source acceptance, signal output, generated artifacts, and
backend status. Differential tests asked the pinned C++ compiler the same
question whenever practical.

### The FIR module verifier

On February 23, the project introduced a verifier for complete FIR modules.
Unlike the partial C++ FIR checkers, it had module-wide context. Its passes
checked:

- module shape and required sections;
- duplicate and missing symbols;
- struct, global, local, loop, and function-argument access classes;
- lexical scope and initialization;
- expression and assignment types;
- function signatures, calls, and arity;
- canonical DSP API contracts.

The verifier turned FIR into a real compiler boundary. **A transform could no
longer hand malformed state or scope relationships to a backend and hope the
generated C++ compiler would diagnose them later.**

See [FIR Module Verifier](../porting/fir-module-verifier-plan-en.md).

### Explicit FIR ownership of the sample loop

The earliest C and C++ emitters synthesized the `compute` loop themselves.
That duplicated execution semantics in each backend. On February 24, loop
ownership moved into FIR. C, C++, and the interpreter then consumed the same
explicit loop rather than independently reconstructing it.

This was a major architectural stabilization: backends became renderers or
lowerers of one execution model instead of alternative sources of scheduling
truth.

### Diagnostics as a compiler-wide API

Diagnostics were treated as another intermediate contract rather than strings
printed wherever an error happened. On February 17, the `errors` crate
introduced a shared model with:

- stable phase-specific codes;
- source spans and labels;
- severity, notes, and actionable help;
- deterministic human and JSON renderings;
- aggregation across parser, evaluation, propagation, and compiler layers.

The parser retained exact source origins through imports. Evaluation errors
could show definition and use sites or alias-resolution traces. Propagation
errors could describe both sides of an invalid composition with their computed
arities instead of returning a generic failure. Human and machine renderings
were locked by snapshot tests from the beginning.

This was especially useful to agents and external tools: a stable code is a
better regression key than English text, and a source range is a better repair
target than a failing phase name. It also forced unsupported backend features
to fail explicitly rather than emit partial output.

On March 15, compilation timeout support was built on cooperative cancellation
rather than terminating the process, keeping the library API safe for embedded
hosts. On July 21, the diagnostic contract was consolidated for automation:
`--check` validated a DSP without producing an artifact, JSON diagnostics used
a clean machine-readable channel, the code table was frozen and documented,
and source/backend families were completed so no surfaced compiler error lacked
a code.

This JSON diagnostic stream is distinct from `-json`: diagnostics describe a
compilation attempt, while `-json` describes a successfully compiled DSP and
its UI.

See [diagnostics model](../porting/faust-rust-diagnostics-model-en.md).

## 8. The interpreter: runtime feedback without a C++ toolchain

The interpreter backend was planned on February 21 and implemented over
February 21–22:

- FBC opcodes and typed instructions;
- a bytecode executor;
- FIR-to-FBC compilation;
- bytecode optimization;
- factory, serialization, instance, UI, and metadata support;
- C and C++ FFI wrappers.

This backend had an outsized effect on the port. Generated C and C++ proved
that the compiler could emit source, but every semantic experiment still
required a host compiler and an executable harness. **The interpreter made the
pipeline directly runnable from Rust**.

Small DSPs could now be compiled, initialized, fed input buffers, and compared
sample by sample. Runtime traces exposed state ordering, delay errors, table
effects, UI behavior, and optimization drift much earlier than source
inspection could.

The project later kept two executor surfaces:

- a fast path for normal execution;
- a checked path returning structured errors for malformed bytecode or tests.

This was another recurring pattern: **first establish a small executable
contract, then add stronger diagnostics and differential runtime evidence
around it.**

## 9. State and time: one simple model first

Delay and recursion are where a DSP compiler's apparent tree structure becomes
temporal state. They were therefore among the first features to expose the
limits of the naïve FIR lowering.

The early fast lane used a small set of direct state shapes. By March 17, delay
and recursion storage had converged on a simpler common mechanism:

- power-of-two circular buffers;
- one persistent `fIOTA` sample cursor;
- masked indexing for delayed reads;
- the same cursor infrastructure for delay lines and recursive history.

Using one model for both concepts reduced the number of temporal invariants
that had to be debugged at once. It made delay reads, recursive projections,
write ordering, and state advancement visible in one sample-indexed model.

This was not the final optimization. **It was a correctness platform.**

## 10. Full signal typing, intervals, and normalization

The first FIR producer used a reduced type view because that was enough to
generate code. The next stage replaced it progressively with the real Faust
signal semantics.

### Prepared signals

On March 9, the compiler introduced a preparation boundary before FIR:

- clone the signal forest into staging storage;
- convert De Bruijn recursion to a symbolic form where needed;
- canonicalize recursive projections;
- infer simple types;
- insert explicit promotions;
- verify the prepared forest.

This prevented the FIR lowerer from becoming a collection of compensating
special cases for every signal spelling produced upstream.

### Full `SigType`

On March 13, the C++ signal type hierarchy and inference rules were ported into
`crates/sigtype`. Rust enums represented simple, table, and tuple types, with
explicit lattices for:

- numeric nature;
- variability (`Konst`, `Block`, `Samp`);
- computability;
- vectorability;
- Boolean properties;
- interval information.

Recursive groups used fixed-point inference instead of ad hoc recursive
guessing.

### Interval arithmetic

The C++ interval library was ported as a first-class crate rather than
approximated inside delay lowering. Interval upper bounds then provided a
generic answer to questions such as “how large can this variable delay become?”

This avoided syntax-specific fixes for sliders, sample-rate expressions, or
`min` wrappers. A delay was accepted when its inferred interval established a
finite non-negative upper bound. When an interval bug appeared, the correct fix
was in interval or type semantics, not in a backend pattern recognizing one DSP.

### Normalization

Algebraic normalization and simplification arrived in stages during March and
became part of the active FIR preparation path in early April. The pipeline
re-ran typing and promotion after simplification because a rewrite can change
both the visible node shape and the required casts.

This repeated `type -> promote -> simplify -> type -> promote` structure may
look conservative, but it kept the FIR boundary explicit and verifiable while
normalization coverage grew.

## 11. From valid code to efficient code: placement and CSE

By early April, the original “everything in the sample loop” strategy had done
its job. The type system now knew the variability of each signal, and FIR was
stable enough to optimize generically.

The runtime optimization work introduced three execution tiers:

| Signal variability | FIR section | Execution frequency |
|---|---|---|
| `Konst` | `instanceConstants` | initialization |
| `Block` | `compute` preamble | once per audio block |
| `Samp` | sample loop | once per sample |

Placement analysis found shared signals and variability boundaries, then
materialized values in the slowest correct tier. Lifetime analysis kept
initialization-only constants local and promoted only values that had to
survive into later functions.

FIR-side common subexpression elimination followed. Hash-consing already made
identical expressions share a `FirId`, but a text backend could still expand
the same FIR subtree at every use. CSE counted uses inside each execution
bucket, created `iConst`/`fConst`, `iSlow`/`fSlow`, and `iTemp`/`fTemp`
materializations, and rewrote repeated uses to loads.

Doing this in FIR rather than independently in C, C++, the interpreter, and
Cranelift meant every backend benefited from the same optimization and the same
verification.

The design and its historical “all expressions in `sample_statements`” baseline
are documented in
[FIR Runtime Optimization](../porting/fir-cse-runtime-optimizations-plan-2026-04-03-en.md).

## 12. Delays and recursion grow into separate, cooperating systems

Once the uniform `fIOTA` model was correct, it became possible to recover the
more efficient storage choices used by Faust C++.

The delay subsystem gained:

- a pre-scan that computes required history and ownership;
- a dedicated `DelayManager`;
- small shift/copy delay lines;
- power-of-two circular buffers using shared `fIOTA`;
- exact-size wrapping buffers with local cursors;
- bounded variable-delay support from interval analysis;
- explicit read, write, and end-of-sample phases.

Recursion was extracted into its own subsystem with:

- canonical group and projection decoding;
- recursion carrier allocation;
- current-value and history access;
- deterministic grouped updates;
- delayed recursion references.

The two concepts were separated in code because they have different semantic
owners, but combined in storage when analysis proved that they represented the
same history. A delayed recursion projection could reuse the recursion carrier
instead of allocating a second independent delay line.

This sequence illustrates the port's general method:

1. use one simple runtime representation to establish semantics;
2. extract analysis from emission;
3. introduce alternative strategies behind a common contract;
4. merge resources only when a general analysis proves equivalence.

See [DelayManager design](../porting/delay-manager-design-2026-04-06-en.md) and
[recent Signal-to-FIR progress](signal-to-fir-recent-progress-en.md).

## 13. Adding C and Cranelift

The previous sections followed the semantic core through its early-April
maturation. Backend and runtime work had started in parallel much earlier.

The C backend followed the C++ backend almost immediately. On February 18, it
was made module-first and consumed the same sine-phasor FIR fixture. This was an
early demonstration that canonical FIR could support more than one text
emitter without duplicating compiler semantics.

Cranelift began one week later, on February 25. Its path was intentionally more
incremental:

- define the DSP struct layout;
- lower state and arithmetic;
- support loops, control flow, tables, and math calls;
- retain JIT modules in factories;
- expose factory and instance APIs through FFI;
- compare runtime behavior with the interpreter.

By February 27, the Cranelift path had real instance execution and differential
runtime tests. It was still an experimental backend, but it proved that FIR was
not tied to textual code generation. The same typed module could drive a JIT,
an interpreter, and C-family source emitters.

## 14. The first operational scalar milestone

The repository began on February 14. By mid-April—roughly two months later—it
had an operational scalar compiler with:

- a substantial parser/evaluator/propagator front end;
- canonical boxes, signals, and FIR;
- signal preparation, full signal typing, intervals, and normalization;
- C and C++ source generation;
- an interpreter runtime;
- an experimental Cranelift JIT;
- delays, recursion, UI, tables, waveform, soundfile, and foreign-symbol slices;
- FIR verification, golden workflows, differential tests, and runtime traces;
- variability placement and FIR CSE;
- multiple delay-generation strategies and recursion/delay resource reuse.

Calling this milestone “equivalent to the official scalar Faust C++ version”
requires a precise scope. It did not mean every backend, every legacy option,
the complete `libfaust` API, vector/work-stealing compilation, or every
historical Faust feature was finished. It meant that the first Rust scalar
version was operationally comparable on the tracked portable scalar corpus and
that remaining gaps were explicit rather than hidden.

The evidence around that milestone was already strong:

- on March 27 there was no tracked case accepted by C++ that failed before the
  Rust signal boundary;
- C and C++ end-to-end compilation had one known valid fast-lane gap;
- the workspace test suite was green;
- early-April work shifted from “make the pipeline exist” to optimization,
  lifetime, delay strategy, and architecture refinement.

That is the meaningful two-month achievement: not that the port was finished,
but that it had crossed from prototype to a coherent compiler whose remaining
work could be expressed as parity gaps, optimization work, backend expansion,
and API completion.

### What the scalar gate adds that the C++ compiler does not have

One departure from the reference deserves to be recorded where a reader of
the port looks for departures, because nothing in the C++ source points at
it. The reference orders scalar code by data dependencies alone:
`compileMultiSignal` builds the immediate-dependency graph and takes its
depth-first schedule, and a foreign function is an ordinary cached
expression, shared by common-subexpression elimination and never ordered
against a delay line or a table. faust-rs offers the same four `-ss`
strategies on the same graph, and a strategy other than depth-first could
move a table write past a read of the same table if only data edges
constrained it. The scalar gate (`transform::signal_fir::build_scalar_gate`)
therefore runs an **effect orientation** first
(`transform::hgraph::orient_effect_conflicts`): the stateful nodes of one
resource are chained in the depth-first baseline order, adjacent pairs only,
so that every strategy sees the same constraints and reorders only commuting
work. The facts come from a signal-level effect analysis, the scalar twin of
the vector plan's.

In that orientation a foreign call is a barrier: Faust has no purity
declaration, every `ffunction` is of unknown purity, and the conservative
reading orders it against every stateful node. This is stricter than the
reference, which assumes purity without saying so; declaring the libm
functions of `maths.lib` pure would remove the barriers and change the
schedules of every program that calls one, and the question is left open
rather than settled by default. The orientation of the barriers was found
cubic in the number of calls on 2026-09-23 (a sum of 320 `ma.tanh` on
delayed reads compiled in 14 s, with or without `rad`) and made incremental
with the edge set unchanged, byte-identical generated code, and a test that
keeps the pairwise definition as its reference. The module header of
`crates/transform/src/hgraph/mod.rs` carries the same account next to the
code.

## 15. Native C and C++ libraries for the interpreter and Cranelift

The two-month milestone above summarizes the compiler core. Its embedding
story had begun earlier: the compiler became an embeddable library almost as
soon as it became a runtime.

On February 22, `interp-ffi` exposed the interpreter through C and C++ APIs
modeled after Faust's `interpreter-dsp-c.h` and `interpreter-dsp.h`. Opaque
factory and instance handles hid Rust ownership from callers while preserving
the important libfaust lifetime rule: a factory must outlive the DSP instances
created from it. The interface covered factory caching, FBC serialization,
instance lifecycle, `compute`, UI construction, and metadata. Because the
native interpreter instance borrowed its factory, the FFI layer used an
explicit wrapper with a raw factory relationship rather than pretending that a
Rust lifetime could be expressed in C.

Cranelift followed on February 25. It began as an API and serialization
scaffold, then acquired retained JIT modules and real instance execution. By
February 27, factories could be created from source, boxes, or signals for the
supported subset, and the C/C++ surface was exercised by runtime comparisons
against the interpreter. Cranelift had no upstream Faust backend to copy
one-to-one, so its external API deliberately followed the familiar
factory/instance organization while its compilation and bitcode contracts were
documented as Rust-specific adaptations.

The two FFI crates soon shared option parsing, factory-cache machinery, error
translation, and C string helpers. A `faust-ffi` facade gathered the exported
symbols into one static or dynamic distribution library. This mattered beyond
code reuse: it established that the Rust compiler could be embedded by an
existing C or C++ host without exposing Cargo crates, Rust enums, or Rust
allocation details.

The distribution library was initially named `libfaust`, matching the
compatibility target. On July 23 it was renamed `libfaust-rs` for native C and
C++ builds so that the Rust and official C++ libraries could be installed or
linked side by side without an ambiguous filename. The exported API contracts
remain deliberately Faust-shaped.

See [Interpreter FFI plan](../porting/faust-rust-ffi-interp-en.md) and
[Cranelift FFI parity matrix](../porting/cranelift-dsp-ffi-parity-matrix-en.md).

## 16. WebAssembly: connecting the port to the Faust web ecosystem

The WebAssembly backend was planned on March 25 and brought up in a concentrated
series of changes on March 26. It was not treated as just another syntax
emitter. Existing Faust web applications depend on a complete binary and
runtime contract:

- a stable linear-memory layout for DSP state, tables, controls, and audio
  buffers;
- the standard DSP lifecycle and parameter-access exports;
- correct single- and double-precision instructions;
- imported mathematical and foreign functions;
- a companion JSON description containing metadata, UI structure, and field
  offsets;
- optional WAT output for inspection and debugging.

The implementation therefore ported the structural responsibilities of the C++
WASM code container into typed Rust components: a layout engine, instruction
lowering, module-section assembly, import/export resolution, data segments,
and a typed JSON builder. The JSON could not be an approximate description:
the JavaScript runtime uses its indices to address the generated module's
memory.

WebAssembly is strategically important to Faust because it is the primary
deployment format for browser DSP, the Faust IDE, Web Audio and AudioWorklet
applications, and the `faustwasm` package. Supporting the backend meant that
`faust-rs` could participate in the existing web ecosystem rather than
creating a separate Rust-only runtime.

That required a second kind of WebAssembly artifact. In addition to compiling
a DSP *to* `.wasm`, the Rust compiler itself was built *as* a
`wasm32-unknown-unknown` module. The resulting compiler service accepts Faust
source and returns WASM/JSON artifacts to `faustwasm`. Standard Faust libraries
were embedded so browser compilation did not depend on a host filesystem, and
mono and polyphonic `faustwasm` paths were validated end to end on March 26.
Later work added soundfile behavior, foreign-function imports, and transport of
auxiliary artifacts such as SVG diagrams through virtual source maps rather
than emulating Emscripten's filesystem.

This dual role—WASM as a DSP target and WASM as the compiler's own deployment
format—made the backend one of the clearest demonstrations that the port was
becoming part of Faust's existing product architecture.

See [WebAssembly backend plan](../porting/wasm-backend-plan-2026-03-25-en.md),
[WASM JSON parity plan](../porting/wasm-json-parity-plan-2026-03-26-en.md), and
[`faustwasm` dual-mode interface plan](../porting/faustwasm-dual-mode-rust-interface-plan-2026-03-26-en.md).

## 17. Machine-readable and visual outputs: `-json` and `-svg`

Two auxiliary outputs made the compiler useful beyond source generation:
`-json` exposes a machine-readable DSP description, while `-svg` exposes the
evaluated block-diagram structure to a human or visual tool. They entered the
port through different compiler layers and at different times.

### Strict Faust JSON

The repository had internal diagnostic JSON in February, but that was not the
public Faust `-json` contract. The real option emerged from the WebAssembly
work on March 26.

WASM needed a companion JSON file whose UI indices, DSP size, sample-rate
offset, metadata, libraries, include paths, and compilation options agreed
with the emitted module's memory layout. Instead of leaving that logic inside
the WASM emitter, the project extracted a typed, generic FIR JSON builder.
That builder then served two related but intentionally different products:

- **strict `-json`**, matching the global C++ Faust description and omitting
  backend-specific widget memory indices;
- **WASM companion JSON**, enriching the same description with the offsets and
  runtime fields required by `faustwasm` and Web Audio hosts.

This separation prevented a convenient WASM implementation detail from
silently changing the public JSON schema. It also gave JSON the same
provenance and UI sources as other FIR backends instead of assembling strings
ad hoc in the CLI.

By the end of March 26, `-json` was a first-class standalone mode writing to
standard output or `-o`. It could also accompany `-lang`: C, C++, FIR,
interpreter, Cranelift, WASM, and WAST output could be written to the requested
path while a matched `.json` file was emitted beside it. Requiring `-o` for
this combined form made the companion path deterministic. On May 5, the
serializer was reformatted into the readable style used by the C++ JSON
backend without changing the typed data model.

This historical `-json` option should not be confused with the structured JSON
diagnostic channel added in July. One describes the compiled DSP and its UI;
the other describes compiler errors and warnings for tools.

### SVG block diagrams

The SVG port began on May 2 with a study of the C++ `compiler/draw/` subsystem:
46 files and roughly 5,400 lines covering schema nodes, layout, drawing
devices, wire collection, folding, and orchestration. The Rust implementation
created a dedicated `draw` crate instead of embedding SVG strings in the
compiler.

The source of an SVG diagram is the **evaluated Box tree**, not Signal IR or
FIR. This preserves the algebra the diagram is meant to explain—sequential and
parallel composition, split, merge, recursion, routes, UI nodes, groups, and
clock-domain wrappers—after abstractions and applications have been resolved
but before signal lowering erases the original block structure.

The May 2 implementation proceeded in layers:

1. a schema tree and drawing-device trait;
2. concrete schemas for leaves, wiring, compositions, recursion, routes, UI,
   groups, and multirate nodes;
3. an SVG device with bottom-up sizing, top-down placement, and a separate wire
   collection pass;
4. Box-to-schema translation and the `-svg`/`--svg` CLI path;
5. visual parity options such as shadows, responsive scaling, route frames,
   and label truncation;
6. hierarchical folding through `-f` and `-fc`.

The CLI follows the C++ directory convention:

```text
faust-rs -svg program.dsp
  -> program-svg/process.svg
  -> program-svg/<folded-subdiagram>.svg ...
```

Folding is not merely an optimization for large images. Named complex
definitions become linked child diagrams, allowing the browser interaction
used by Faust tools: enter a block to inspect its definition, then navigate
back to its parent. Preserving definition names through evaluation was
therefore a compiler requirement, not a cosmetic draw-layer fix.

On May 3, `generateAuxFiles` made `-svg` and `-json` available through the
compiler facade and the interpreter/Cranelift C and C++ APIs. On May 4, SVG
rendering gained a filesystem-free path. The embedded Rust compiler used by
`faustwasm` can return the complete SVG hierarchy as an ordered artifact map,
with `process.svg` first and every relative link preserved. Virtual source
injection also ensures that diagrams for DSPs importing user libraries can be
generated in a browser without an Emscripten filesystem.

Together, the two options opened complementary inspection surfaces:
structured JSON for hosts and automated tools, and navigable SVG for people,
documentation, IDEs, and agents reasoning about DSP topology.

See [SVG draw port plan](../porting/draw-svg-port-plan-2026-05-02-en.md) and
[`faustwasm` SVG auxiliary-files plan](../porting/faustwasm-svg-aux-files-plan-2026-05-03-en.md).

## 18. FAD and RAD: automatic differentiation becomes a compiler feature

Automatic differentiation was the first major feature whose semantics were not
simply recoverable from the pinned production C++ compiler. `fad` and `rad` are
`faust-rs` extensions: the C++ reference used for the port does not recognize
them. Their implementation consequently combined historical Faust signal
semantics with new, explicitly documented differentiation contracts.

### Forward mode

Forward-mode automatic differentiation started on April 13 with parser
wrappers and a propagation-stage transform. The earliest prototype
automatically discovered UI controls. That was useful for proving that dual
signal propagation worked, but it made the differentiation variable implicit
and made nesting, recursion, and library composition harder to reason about.

On April 15, the public form was deliberately changed to:

```faust
fad(expr, seeds)
```

The second argument makes the differentiation variables explicit. If `expr`
has `M` outputs and `seeds` has `N` lanes, the result is interleaved as each
primal output followed by its `N` tangents, for a total arity of
`M * (1 + N)`. Seed identity is structural signal identity, not a label lookup
or a backend convention.

The transform then grew from elementary arithmetic into a real signal
transformation:

- local derivative rules for arithmetic, casts, comparisons, selections, and
  mathematical functions;
- explicit seed lifting through nested scopes;
- De Bruijn-native differentiation, so recursive signals did not have to be
  converted prematurely to another representation;
- recursion carriers augmented with tangent state;
- multiple outputs and multiple tangent lanes;
- readonly table and waveform differentiation;
- runtime and corpus cases for filters, adaptive effects, Newton solvers, and
  recursive learning loops.

The key recursion decision was to keep primal and tangent state in the same
augmented recursive group. This prevented two nominally related recursions from
drifting in schedule or delay semantics.

### Reverse mode

RAD implementation began on April 27. The first phase established
`rad(expr, seeds)` parsing and arity, followed by a symbolic reverse sweep for
feed-forward graphs and derivative rules for the extended primitives. Stateful
graphs were initially a loud boundary rather than a source of plausible but
incorrect gradients.

Work from late April through early May explored linear time-invariant
transposition and reverse-time recursion. That path successfully handled
selected delays and recursive filters, but it was too narrow to remain the
correctness path for general nonlinear state. The more general
`BlockReverseAD` carrier was therefore introduced:

1. execute the primal forward over the current `compute(count)` block and
   record the residual values needed by the reverse rules;
2. execute an adjoint sweep backward over that block;
3. use zero terminal adjoint state at the block boundary.

This is an explicit truncated backpropagation-through-time contract. It does
not claim an infinite-horizon gradient, and the per-sample gradient lanes are
contributions over the block rather than an already reduced scalar.

On May 12, the specialized `ReverseTimeRec` dispatcher fast path was disabled.
The lowering infrastructure remained tested, but general RAD correctness was
routed through `BlockReverseAD`. This is a revealing historical choice: the
project preferred a slower general model with a clear temporal contract over a
fragile optimization that worked only on selected recursions. A May 21 design
then proposed “linearize once, then transpose” as the longer-term way to share
local rules between FAD and RAD while retaining block scheduling and tape
semantics in FIR.

See [FAD and RAD synthesis](fad-rad-synthesis-en.md),
[FAD note](fad-note-en.md), [RAD note](rad-note-en.md), and
[linearize-once RAD plan](../porting/rad-linearize-once-transpose-plan-2026-05-21-en.md).

## 19. Julia: the first new source backend after the scalar core

The Julia backend landed on May 13, with numeric-cast parity refinements on May
14. It was built module-first from canonical FIR rather than from a
Julia-specific signal compiler.

The emitter reproduced the recognizable shape of Faust's C++ Julia output:

- a typed mutable DSP structure;
- metadata, UI, and lifecycle functions;
- `compute!` over Julia matrices;
- one-based Julia array indexing around zero-based FIR loop and table indices;
- single- and double-precision aliases;
- architecture-file wrapping;
- explicit diagnostics for unsupported FIR nodes.

Early corrections aligned UI zones with the C++ backend's symbol-based field
access and made numeric casts preserve C-family semantics in Julia. The result
was intentionally described as a functional first slice, not full byte-for-byte
or runtime parity. Its architectural value was nevertheless immediate: it
showed that FIR could drive a high-level, garbage-collected, one-based target
without pushing target conventions back into the signal compiler.

See [Julia backend plan](../porting/julia-backend-plan-2026-05-13-en.md).

## 20. Exporting the Box and Signal APIs

The runtime FFI did not yet provide the other major role of libfaust:
programmatic compiler construction. External tools need to build and inspect
Box and Signal graphs, convert boxes to signals, normalize signals, and
generate source without first serializing everything as Faust text.

The first Box C/C++ layer landed on February 27. On June 9, this became a
systematic Box/Signal API parity project against:

- `libfaust-box-c.h` and `libfaust-box.h`;
- `libfaust-signal-c.h` and `libfaust-signal.h`.

The internal Rust API remained deliberately idiomatic: `BoxBuilder` and
`SigBuilder` construct nodes, while `match_box` and `match_sig` provide typed
structural views. The external boundary mirrors the established libfaust
naming and ownership rules:

- opaque Box and Signal handles;
- `Cbox*`, `Csig*`, `CisBox*`, and `CisSig*` functions;
- context creation and destruction;
- null-terminated result arrays and `freeCMemory`;
- Box-to-Signal conversion, normal form, printing, arity/type queries, and
  source generation;
- thin C++ overload wrappers over the stable C ABI.

Source generation through this API is deliberately narrower than the CLI. At
the cutoff, `CcreateSourceFromBoxes` and `CcreateSourceFromSignals` accepted
`c`, `cpp`, `fir`, and `interp`, and rejected any other language with a typed
error. The later Julia, WebAssembly, AssemblyScript, and Rust emitters are
reached through the CLI and the compiler facade, not through the
libfaust-compatible Box/Signal entry points.

The implementation extracted a shared `tree-ffi` context so Box and Signal
handles use one arena, allocation pool, and provenance model. A dedicated
`signal-ffi` crate then filled construction, recursion, table, waveform,
soundfile, UI, matcher, and source-generation families. Rust-only extensions
such as `boxFad` and `boxRad` were exported explicitly rather than disguised as
C++ parity.

This work was matrix-driven. Reference headers were inventoried symbol by
symbol, C11 and C++17 smoke clients included the generated headers in both
orders, and `xtask libfaust-export-check` compared actual dynamic-library
exports with declarations. The June 9 validation found all 269 declared
Box/Signal C symbols in the produced library.

The result gave `faust-rs` two complementary public entry points: compile Faust
text through the compiler facade, or construct the compiler's tree languages
directly through a libfaust-compatible API. This is particularly important for
visual tools and agentic DSP workflows, where a structured graph is safer to
inspect and mutate than source text.

See [Box/Signal API parity plan](../porting/libfaust-box-signal-api-parity-plan-2026-06-09-en.md).

## 21. AssemblyScript: a source-to-WASM bridge

The AssemblyScript backend landed on June 10 as `-lang asc`. It is distinct
from the binary WebAssembly backend: instead of encoding WASM instructions
directly, it emits typed AssemblyScript source that the `asc` compiler then
translates to WebAssembly.

The implementation followed the C++ Faust AssemblyScript generator but used
the same module-first FIR boundary as the other Rust backends. Its public shape
included:

- an exported DSP class with instance and static state;
- `StaticArray<T>` storage and explicit numeric casts;
- `Math`/`Mathf` selection for single and double precision;
- FIR lifecycle and `compute` methods;
- optional embedded `getJSON()`;
- typed backend errors for unsupported FIR nodes.

The backend was connected immediately to the compiler CLI and to
`generateAuxFiles`, including the process-name selection used by `faustwasm`
transpilation requests. This allowed web tooling to request AssemblyScript
source as an auxiliary artifact rather than requiring the direct binary WASM
path.

On June 15, AssemblyScript joined the impulse-test system. A Node runner:

1. compiled the DSP with `faust-rs -lang asc`;
2. wrapped the generated class with buffer and control entry points;
3. invoked AssemblyScript 0.27 to produce a WASM module;
4. executed that module under Node;
5. emitted the common scalar impulse format.

The first gate matched 66 of 93 programs and made the remaining gaps concrete:
double precision had not been threaded through the emitter, several C-style
math helpers needed AssemblyScript implementations, and soundfile loads needed
host imports. Those generic fixes were completed the same day. The
AssemblyScript-specific known-failure list then became empty; only the
suite-wide source compilation exclusion remained.

This backend reinforced two recurring lessons. Canonical FIR made a new target
cheap to bring up, while the shared runtime oracle prevented “generated source
compiles” from being mistaken for DSP parity. It also gave the web ecosystem a
choice between direct WASM emission and a readable/transpilable
AssemblyScript intermediate.

See the AssemblyScript implementation entries for
[June 10](../porting/journal/2026-06-10.md) and
[June 15](../porting/journal/2026-06-15.md).

## 22. Rebuilding `impulse-tests` as a systematic backend gate

The C++ Faust repository already had an unusually valuable end-to-end oracle:
`tests/impulse-tests`. Each DSP is executed for 60,000 frames in a four-pass
protocol:

1. a scalar impulse run;
2. the same run with random block splitting, checking block-boundary
   invariance;
3. a four-voice polyphonic run;
4. a one-voice polyphonic run.

The protocol fixes sample rate, block size, initialization, button behavior,
double precision, output formatting, zero normalization, and numeric
tolerance. It tests much more than an impulse response: lifecycle, cloning,
polyphony, UI defaults, state continuity, and independence from how a host
partitions its blocks.

The Rust port first studied this mechanism on June 14, then landed its harness
on June 15. The central decision was to preserve the genuine C++ executable as
the oracle rather than regenerate “expected” files through the Rust compiler.
The 93 DSP inputs were brought into a backend-neutral directory, while the
original C++ architecture headers remained referenced from an overridable
checkout instead of being copied into the Rust repository.

Two populations must not be confused in the numbers that follow. **93** is the
imported C++ impulse population, and it is the denominator for C++-oracle
comparison and for the per-backend impulse gates. The same directory later also
received Rust-extension and clock-domain DSPs, so the **corpus** it holds had
grown to 133 programs by the July 23 cutoff; that larger number is the
denominator for Rust-only measurements such as vector admission.

The new harness made the original idea more systematic across backends:

- shared configuration and predictable artifact directories;
- one driver per backend;
- exact full four-pass comparison for generated C and C++;
- scalar-prefix comparison for in-process runtimes that did not yet implement
  the polyphonic wrapper;
- per-DSP tolerance declarations separated from known unsupported cases;
- a written cause for every expected failure;
- green targets whose coverage count could only grow when a real backend bug
  was fixed;
- benchmark targets over the same DSP population.

This structure avoided two common test-suite failures: silently dropping cases
that do not pass and loosening one global tolerance until every backend looks
green. A mismatch was classified as rounding, unsupported capability, compile
failure, runtime failure, or semantic divergence, with the exact backend and
first failing behavior kept visible.

The suite immediately paid for itself. It exposed reverse-loop handling in the
interpreter, `instanceClear` and precision problems in Cranelift, double-literal
precision in C, and lifecycle differences shared by several backends. On the
first full run, the Rust-generated C++ backend matched 92 of 93 C++ reference
programs over all 60,000 frames. Interpreter and Cranelift gaps became
actionable runtime bugs instead of vague compiler failures.

WASM and AssemblyScript runners joined the gate the same day. Later additions
covered scheduling and vector variants, clocked DSPs, and the Julia and Rust
backends. The Rust backend reached a 92/92 green gate on July 17. Thus
`impulse-tests` evolved from a C++ backend test into a common executable
contract for every backend that could implement the relevant lifecycle.

See [Impulse-test harness port plan](../porting/impulse-tests-harness-port-plan-2026-06-14-en.md).

## 23. Porting `ondemand`, `upsampling`, and `downsampling`

Clock-domain work began with a C++ source study and port plan on June 10. Unlike
ordinary Faust signals, the three primitives let a subgraph advance at a
different logical rate:

- `ondemand(C)` executes `C` zero or one time for a Boolean-range clock, or a
  clock-specified number of times for an integer-range clock;
- `upsampling(C)` executes `C` several times per outer sample;
- `downsampling(C)` executes `C` only every requested number of outer samples.

All three add a clock input to the wrapped DSP. Their defining invariant is
local time: delays, recursion, tables, waveforms, and state inside the domain
advance on domain firings, not automatically on outer audio samples. Outputs
are held between firings, and `upsampling` uses explicit boundary behavior such
as zero padding where required.

Implementation resumed in July after the analysis had fixed the ownership
model. On July 7, prepared-signal processing was changed to preserve clocked
graphs rather than flattening or misinterpreting their environment markers.
Clock-environment inference then built a hierarchy of top-rate and nested
domains. On July 8, Boolean `ondemand` lowering was followed by integer
`ondemand`, upsampling, and downsampling; per-domain `fIOTA`, circular
recursion carriers, held payloads, and fire-time waveform advancement followed
in the same sequence.

The port did not model a domain as an ordinary expression guarded by an `if`.
It introduced explicit boundary and ownership nodes—snapshot inputs, guarded
or repeated bodies, permanent held outputs, zero padding, and domain-local
state. This prevents an optimizer or backend from accidentally advancing state
while the domain is inactive.

The new model also enabled frame-rate DSP. `interleave.lib` serializes a stream
into parallel frames, runs an FFT/STFT body under an `ondemand` frame clock,
and serializes the results back. July corpus additions grew from small
round-trip FFTs into filters, convolution, phase-vocoder, and denoising
examples. These were not special spectral nodes: they exercised the general
clock-domain contract.

See [clock-domain analysis and port plan](../porting/ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md)
and [user-facing clock-domain note](ondemand-note-en.md).

## 24. How clock domains compose with FAD and RAD

There was no C++ oracle for the combination because the pinned C++ compiler did
not provide FAD or RAD. The project therefore separated the work into explicit
semantic phases instead of allowing generic tree traversal to guess.

The first rule was conservative: differentiation strictly *inside* one clock
domain is valid because the tangent or adjoint operations run under the same
local clock as the primal. Crossing a domain boundary is different. An early
FAD catch-all preserved the primal but assigned zero tangents to unfamiliar
clock nodes. That would have produced a compiling DSP with silently false
gradients, so the July 7 groundwork replaced it with a loud boundary
diagnostic.

FAD Phase A, validated on July 8, allowed `fad` inside a domain. Phase B,
completed on July 10, defined structural boundary rules. Under the assumption
that the clock itself is not differentiated, snapshot, hold, and zero-pad
operators are linear time-varying operations, so differentiation commutes with
them. The implementation uses **block augmentation**:

```text
one clocked block carrying primal lanes
  -> one augmented clocked block carrying primal and tangent lanes
```

It does not create a primal block and a separate tangent block. Running two
blocks would advance domain-local recursion and time twice. Every consumer is
rewired to the one augmented block, which reuses the same clock environment
and holds primal and tangent outputs together. This design made differentiable
STFT losses and other frame-rate learning cases possible without weakening
fire-time semantics.

Clock-dependent firing instants remain a documented non-differentiable control
boundary: derivatives through the discrete decision are ignored, just as for
selectors and integer casts. The policy is explicit rather than accidental.

RAD remains more restrictive. Transposing a rate-changing or event-triggered
domain requires a clock-aware tape, reverse scheduling over firing events, and
well-defined adjoints for snapshot, hold, decimation, and repetition. The
current design therefore keeps a loud rejection for unsupported RAD crossings
instead of reusing an audio-rate `BlockReverseAD` tape with the wrong time
base. The intended order is to establish the forward clock model and FAD block
augmentation first, then add clock-aware reverse mode.

This staged relationship captures a broader design principle of the port:
composable new features are not declared compatible merely because their node
types can be nested. Their time, state, and ownership contracts must compose as
well.

See [FAD/RAD and clock-domain cohabitation](../porting/ondemand-fad-rad-cohabitation-2026-06-10-en.md)
and [differentiable STFT status](../porting/fad-phase-b-s4-differentiable-stft-status-2026-07-09-en.md).

## 25. A new model for vector compilation

Vector-mode analysis started alongside the clock-domain study on June 10, and
implementation began on July 10. The user-facing options preserve the Faust
vocabulary:

- `-vec` selects vector compute mode;
- `-vs N` selects the chunk size, 32 by default;
- `-lv 0` emits a constant-trip main loop plus a scalar remainder;
- `-lv 1` emits one simpler loop with a runtime-bounded final chunk.

The option is `-lv`, not `-el`: “loop variant” is the name inherited from
Faust C++.

The first implementation proved the chunk driver, cross-loop buffers, and
recursive-tail separation in FIR. That prototype also exposed the limit of a
late FIR pass: it had to reconstruct signal dependencies, execution
conditions, delayed uses, and recursion relationships after much of that
information had already been fused into statements.

The production design moved vector decisions back to prepared signals. It
separates three responsibilities:

1. signal-level analysis creates an immutable `VectorPlan` describing loop
   ownership, inlined values, dependencies, effects, execution epochs, and
   cross-loop transports;
2. a scheduler serializes the resulting loop DAG without changing its
   boundaries;
3. signal-to-FIR lowering materializes the checked regions, buffers, and state
   transitions.

This preserves the semantic ideas found in C++—occurrence analysis,
`needSeparateLoop`, recursion loops, dependency edges, and transport
selection—but changes the trust boundary. C++ discovers loops incrementally
while lowering and immediately emits the associated storage. Rust first
produces a finite plan, then asks an independent checker to re-derive its
invariants before code generation. Schedule and vector-plan certificates cover
structural facts such as dependency order, unique ownership, transport
completeness, effect barriers, and clock compatibility. Runtime traces,
final-state comparisons, C++ differentials, and impulse tests remain necessary
for semantic evidence; the certificates are not described as proofs of
floating-point behavior.

There was an important limit to executable C++ comparison: the pinned
reference branch rejected `-vec` at its command-line validation boundary.
Consequently, the C++ vector implementation could be studied as source and its
individual rules could be ported, but it could not provide a generated
vector-topology oracle for this branch. Scalar `-ss` scheduling retained a
direct differential target; vector execution relied on the native C++ impulse
oracle for numerical behavior and on explicit admission/fallback accounting
for plan coverage.

### Lean as an explicitly experimental assurance layer

On July 11, the project added a Lean 4 model for the finite structural core of
vector scheduling. This was an experiment, not a decision to formally verify
the whole Faust compiler.

The Lean file defines abstract rates, dependency graphs, schedule validity,
the four scheduling strategy tags, the ordered `needSeparateLoop` rule,
placement, effects, epochs, typed transports, and selected lockstep/fission
obligations. It contains executable Boolean checkers and kernel-checked lemmas
for small algebraic properties. It deliberately leaves compiler-specific
statements—most importantly scalar/vector semantic equivalence—as explicit
propositions or future proof obligations. Compiling the file proves that its
definitions and theorem bodies are accepted by Lean without `sorry` or axioms;
it does **not** prove that the Rust producer implements those definitions.

The experiment explored a producer/certificate/checker workflow:

```text
complex Rust producer
  -> finite canonical certificate
  -> small independent Rust checker
  -> optional Lean re-check of the same artifact
```

The active value of this work was primarily architectural. It forced schedule
edge direction, certificate contents, refusal cases, and assurance vocabulary
to be stated precisely. Rust checkers were implemented independently of their
producers, and mutation tests verified that reversed dependencies, missing
transports, invalid effects, or inconsistent ownership were rejected.

The boundary must be stated honestly. At the July 16 implementation review:

- Rust producer/checker gates were active for several vector stages;
- the Lean model and selected fixtures compiled and cross-checked;
- canonical certificate export and hashing were incomplete;
- a Lean checker consuming every exported Rust certificate in CI was still
  future work;
- no refinement theorem connected the complete Rust implementation to the
  Lean model;
- scalar/vector sound equivalence remained supported by bit-exact and numeric
  differential tests, runtime traces, and backend execution—not by Lean.

Lean was therefore a useful but genuinely experimental method for investigating
whether small, high-risk, finite compiler decisions could gain more assurance
than ordinary testing. It was not applied to everyday parser or backend work,
was not on the critical path of the scalar compiler, and must not be cited as a
proof of `faust-rs`, vector code generation, floating-point equivalence, or the
Faust language semantics.

See [Lean/Rust certified-porting experiment](../porting/lean-rust-certified-porting-plan-2026-07-11-en.md)
and [Lean vector scheduling specification](../porting/vector-mode-scheduling-formal-spec.lean).

The failure policy is equally important. An unsupported effect, clocked-state
shape, or uncertified route fails closed to scalar lowering with a specific
reason. The compiler must not keep vector mode by emitting a plausible
schedule that the checker cannot justify.

The Rust model also simplifies scheduling options. In the studied C++ path,
`-ss` schedules scalar signal graphs, while vector loops have a separate
default level ordering and optional `-dfs`. `faust-rs` gives `-ss` one meaning:
it orders the active scalar graph or vector `LoopGraph`, but it never changes
the `VectorPlan` itself. This is an intentional adaptation, not a claim of
byte-for-byte scheduling parity.

Finally, the project added an extension beyond ordinary block fission:
lockstep vectorization. Independent recursive instances with isomorphic
structure cannot be parallelized through time, but they can occupy SIMD lanes
and advance one sample at a time together. Detection, state ownership, event
ordering, and lane fusion are checked before one physical FIR loop is emitted.
Near-isomorphic or state-coupled groups fall back instead of being forced into
a bundle.

By mid-July, checked vector lowering covered pure regions, recursive delays,
UI, tables, soundfiles, clocked regions, AD policy, C/C++ generation, runtime
backends, and expanding impulse matrices. Admission was then tracked as a
regenerable baseline rather than a prose claim. The July 16 implementation
review measured 49 of the 93 impulse programs as actually admitted to
certified vector plans; after the coverage recovery work, the baseline
regenerated on July 22 measured **97 of 133 corpus DSPs certified, identically
in all 16 vector modes**, with 35 explicit fallbacks (13 plan, 11 clock/AD, 7
event, 4 pure) and 1 error. The remaining cases fell back explicitly. The
larger all-green backend matrices established semantic correctness for
requested vector modes, but must not be read as every corpus DSP executing
vectorized code. The significant innovation was not the chunk loop itself; it
was making vectorization a typed, inspectable, fail-closed compiler decision
whose coverage is a checked-in artifact
([`tests/vector-coverage/corpus-baseline.json`](../tests/vector-coverage/corpus-baseline.json)).

See [vector signal-level analysis and port plan](../porting/vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md),
[reader-oriented vector synthesis](vector-scheduling-synthesis-en.md), and
[implementation review](../porting/scheduling-vectorization-implementation-review-2026-07-16-en.md).

## 26. Rust as a generated-code backend

The fact that the compiler is written in Rust does not automatically make Rust
a target language. That separate backend arrived on July 17.

Like Julia and the other mature emitters, the Rust backend consumes canonical
FIR modules. It generates the public contract expected by the official Faust
Rust architectures: host-supplied `F32`, `F64`, `FaustFloat`, `Meta`, `UI`,
`ParamIndex`, and `FaustDsp` types and traits. It does not hide the DSP inside
a private faust-rs runtime, because generated files must remain usable by
existing `faust2jackrust -source` and `faust2portaudiorust -source` projects.

Rust required explicit preservation of several C semantics:

- wrapping integer arithmetic where generated C relies on two's-complement
  behavior;
- explicit numeric conversions;
- zero-based FIR table indices converted at the Rust indexing boundary;
- borrow-safe construction of disjoint mutable output channel slices;
- standard lifecycle and UI behavior.

The first emitter was followed immediately by an impulse-test target, which
reached 92/92 on July 17. July 18 removed dependencies on locally installed
Faust libraries from tests, aligned the generated runtime contract more closely
with the C++ Rust backend, and expanded Julia/Rust impulse matrices. On July 21
the backend was exposed through the compiler facade as well as the CLI, together
with AssemblyScript. That facade is the `compiler` crate's programmatic entry
point; the libfaust-compatible Box/Signal source generators kept their narrower
`c`/`cpp`/`fir`/`interp` language set.

The Rust backend closes an important loop in the project's history: the same
typed FIR that helped port Faust *to* Rust can now emit DSP code *in* Rust for
the pre-existing Faust Rust architecture ecosystem.

See the [July 17 implementation journal](../porting/journal/2026-07-17.md).

## 27. Growing the corpus, fixing the compiler everywhere

The corpus was not a final acceptance suite added after implementation. It was
the development engine.

It grew from a small representative set into:

- parser and signal fixtures;
- valid and invalid DSP pairs;
- recursive and delayed programs;
- UI, table, waveform, soundfile, and foreign-function cases;
- runtime traces;
- backend-specific contract fixtures;
- eventually hundreds of portable and Rust-extension DSPs.

The growth is visible in historical snapshots:

- early parser work reached the `rep_30` range during the first days;
- the sine-phasor fixture reached `rep_38`;
- March 14 reported 71 of 72 valid end-to-end cases;
- March 24 reported 82 of 83, with the same known stream-wrapper gap;
- March 27 tracked 104 compiler corpus cases and more than 1,000 tests;
- the May assessment counted 194 corpus inputs, about 1,387 tests, and 93 of 94
  portable C++-accepted backend cases;
- by July 23 the impulse corpus alone held 133 DSPs, the 93 imported C++
  programs plus Rust-extension and clock-domain additions.

Each new DSP could fail in a different layer. AI agents were used to inspect
and modify parser, evaluator, propagation, signal typing, normalization, FIR,
backend, or runtime code as needed. That cross-layer speed was one of the
project's greatest advantages, but also its greatest quality risk.

The safe workflow became:

1. minimize or select a DSP exposing one divergence;
2. compare Rust and C++ at the earliest observable boundary;
3. locate the first stage whose output differs;
4. correct the general rule owned by that stage;
5. add a focused unit test and a corpus/differential regression;
6. run formatting, Clippy, workspace tests, and golden checks.

In this workflow, AI supplied implementation velocity and broad search
capacity. The maintainer supplied the semantic oracle, architectural choices,
and refusal criteria.

## 28. The rule against patches

Fast AI-assisted development can easily produce code that makes one failing DSP
green while weakening the compiler.

The project therefore adopted a strong rule: reject “rustines”—local patches
for isolated examples—and prefer the general compiler correction even when it
requires work in a lower layer.

Examples of the intended distinction include:

- fix interval semantics instead of recognizing one `ma.SR` delay expression;
- fix evaluation arity or closure behavior instead of special-casing one
  library definition;
- canonicalize a signal form before FIR instead of teaching every backend a
  new spelling;
- fix ownership and lifetime in FIR instead of emitting a backend-local
  temporary;
- analyze delay and recursion resources instead of matching one syntactic
  feedback pattern;
- add a typed unsupported-feature diagnostic instead of silently generating a
  stub.

This principle also shaped reviews of AI-generated code:

- Does the change name the C++ source or semantic invariant it ports?
- Is the fix located in the stage that owns the rule?
- Does it apply to a family of equivalent programs?
- Is there a structural or differential test that would catch regression?
- Does it preserve lifecycle, type, and cross-platform contracts?

A later repository-wide quality assessment characterized the result as a
mature, unusually disciplined port with a production-quality core, while still
identifying structural refactoring debt in several very large files. That is
an important balance: generic correctness does not imply that the first
implementation structure is the final maintainable structure.

See [Implementation Quality Assessment](../porting/faust-rs-code-quality-assessment-2026-05-25-en.md).

### What the AI experiment actually demonstrated

The initial effort study predicted that AI would accelerate mechanical porting,
test generation, documentation, and broad code search much more than
architecture or DSP semantics. The implementation history largely confirmed
that split.

AI agents were particularly effective at:

- inventorying large C++ subsystems and tracing call paths across files;
- generating repetitive builder, matcher, enum, opcode, and backend families;
- preparing focused plans and source-provenance notes before implementation;
- adding unit, corpus, and differential fixtures with each small tranche;
- searching horizontally across parser, evaluator, signal, FIR, and backend
  layers when one DSP exposed a cross-cutting bug;
- performing cleanup and module extraction once an oversized first
  implementation had stabilized.

The four-day executable compiler spine and the two-month scalar milestone are
strong evidence that this acceleration was real. The agents did not have to
wait for the complete compiler: hand-built FIR fixtures, the interpreter, and
co-located Rust tests created short feedback loops in which one incomplete
layer could be developed against another.

The experiment also showed what AI did **not** replace:

- choosing the semantic reference when several C++ paths existed;
- deciding which compatibility surfaces could be adapted and which had to be
  reproduced exactly;
- understanding DSP time, causality, recursion, lifecycle, and numerical
  behavior;
- deciding whether a fast path was trustworthy enough to remain active;
- rejecting an attractive local fix when it violated a general invariant;
- calibrating claims such as “parity”, “certified”, or “proved”.

**The maintainer remained the architectural and semantic authority. Agents
proposed options and implemented bounded steps; executable C++ behavior,
explicit plans, and maintainer decisions determined which option became the
project contract.**

Several characteristic AI failure modes appeared during the port:

- code that compiled and passed a narrow test but changed a nearby lifecycle or
  arity rule;
- duplicated helper ladders instead of extending the canonical builder/matcher
  API;
- a backend workaround for an error owned by typing or FIR;
- optimistic “supported” claims based on one generated source file;
- large first-pass modules that later required deliberate factorization;
- documentation or status tables that became stale as implementation moved
  quickly.

The response was process, not better prompting alone: small coherent commits,
Rustdoc provenance, daily journals, explicit handoffs, typed unsupported
diagnostics, unit tests at the owning layer, and broader differential/golden/
impulse gates before declaring completion.

The concrete conclusion is therefore conditional but positive. AI agents made
a significant compiler port, cleanup, re-architecture, and feature-development
effort progress much faster than a conventional sequential implementation.
They did so reliably only inside a workflow where a domain expert controlled
the architecture, the reference behavior was executable, and every local
success had to survive progressively wider validation. This was successful
AI-assisted engineering, not autonomous compiler construction.

The estimates and original division of responsibilities are recorded in
[the effort assessment](../porting/faust-rust-bilan-effort-en.md).

## 29. State of the port on July 23, 2026

The density of this history should not be read as a claim that every C++ Faust
feature had been reproduced. By July 23, `faust-rs` had a production-quality
core and a wide executable surface, but maturity differed by subsystem.

### Operational and strongly validated

- The canonical scalar pipeline was operational from source through FIR to C
  and C++ on the tracked portable corpus.
- Parser, evaluation, propagation, typing, intervals, normalization, delays,
  recursion, UI, tables, waveform, soundfile, and foreign-function families had
  broad unit, corpus, golden, and differential coverage.
- The interpreter and Cranelift provided in-process execution; their runtime
  behavior was checked against each other and the impulse oracle.
- Architecture wrapping and Faust-shaped lifecycle contracts allowed generated
  C/C++ and Rust code to remain usable by existing host architectures.
- Box and Signal APIs, interpreter/Cranelift factories, and native C/C++
  headers were available through `libfaust-rs`.
- `impulse-tests` provided a shared executable backend contract rather than
  relying only on generated-source inspection.

“Operational scalar equivalence” means this tested production-oriented
envelope. It does not mean that every historical option, auxiliary compiler
path, public API overload, standard-library construction, or backend matched
C++ Faust.

### Rust extensions

Several features intentionally went beyond the pinned C++ reference:

- Cranelift as a Rust-native JIT backend;
- `fad(expr, seeds)` and `rad(expr, seeds)`;
- `BlockReverseAD` and its explicit block-local TBPTT semantics;
- FAD block augmentation across clock domains;
- one scheduling option shared across scalar and vector modes;
- checked `VectorPlan` boundaries and lockstep vectorization;
- the experimental Lean specification and certificate methodology.

These extensions use numerical, structural, or internal reference oracles
where no C++ behavior exists. They must not be described as C++ parity
features.

### Functional but not uniformly mature

| Surface | Status at the cutoff |
|---|---|
| WebAssembly | Functional with companion JSON and validated `faustwasm` mono/poly paths; full C++ WASM semantic and layout parity was not claimed |
| AssemblyScript | Real FIR backend with `asc` and impulse execution; still dependent on the external AssemblyScript toolchain and its runtime contract |
| Julia | Useful module-first emitter with precision, UI, casts, and architecture wrapping; still an initial slice assuming a host Julia Faust runtime |
| Rust | Architecture-compatible source backend with a 92/92 impulse gate; breadth beyond the exercised FIR/corpus surface remained subject to expansion |
| Vector mode | Checked and executable on covered plans; the July 22 baseline measured 97/133 corpus DSPs certified in all 16 vector modes, while unsupported or uncertified shapes fell back to scalar; profitability was not yet automatically modeled |
| Lean | Experimental specification/checker research; no proof of the Rust compiler or scalar/vector semantic equivalence |

### Explicit gaps, policies, and exclusions

- Remote URL imports and the broader C++ `sourcefetcher` behavior remained
  deferred; native local imports and WASM virtual sources were the supported
  models.
- The embedded compiler helper surface was incomplete in places such as full
  `getInfos` and legacy filesystem semantics.
- Specialized `ReverseTimeRec` RAD dispatch remained disabled; general
  temporal RAD used `BlockReverseAD`, and unsupported RAD crossings of clock
  domains failed loudly.
- Vector scheduling still had cost-model, topology-oracle, certificate-export,
  and Lean-in-CI follow-up work.
- External control-rate separation (`-ec`) and one-sample execution (`-os`) had
  a documented porting plan but were not yet an implemented historical
  milestone at this cutoff. See the
  [external-control and one-sample plan](../porting/external-control-one-sample-port-plan-2026-07-23-en.md).
- LLVM backend parity had not been delivered; Cranelift was the active
  Rust-native JIT experiment.
- The Java backend and legacy `-lang ocpp` mode were explicitly outside the
  port target.
- Long-tail behavior outside the tracked corpus remained unproven even when its
  node family existed in Rust.

This status distinction is part of the porting discipline: a feature can be
implemented, executable, and useful without being labeled complete or
parity-equivalent.

## 30. Condensed timeline

| Date | Milestone |
|---|---|
| 2026-02-14 | C++ inventory, phased plan, Cargo workspace, three-OS CI, and dual Rust/C++ golden workflow |
| 2026-02-15 | `TreeArena` benchmarks; boxes/parser prototype; `SourceReader`, local imports, cycle detection, and parser differential harness |
| 2026-02-16 | Canonical signal representation; evaluation, propagation, recursion; first `parse -> eval -> propagate -> signals` path |
| 2026-02-17 | Structured diagnostics with codes/spans; canonical FIR; module-first C++ backend; AI-built sine/phasor fixture |
| 2026-02-18 | Signal-to-FIR fast lane; first source-to-C/C++ chain; C backend; UI, state, delay, table, and recursion slices |
| 2026-02-19 | C/C++ architecture wrapping (`enrobage`), search paths, inline includes, markers, and differential fixtures |
| 2026-02-21–22 | Interpreter bytecode, executor, optimizer, factory, serialization, runtime, and C/C++ FFI |
| 2026-02-23–24 | FIR module verifier; FIR-owned explicit sample loop; runtime trace infrastructure |
| 2026-02-25–27 | Cranelift JIT and FFI bring-up; Box FFI; unified native library; interpreter/Cranelift differential runtime checks |
| 2026-02-28 | Imported-source provenance, used-source reporting, parser diagnostic hardening, and explicit remote-import policy |
| 2026-03-09 | Prepared-signal boundary, initial signal typing, promotion, and delay cleanup |
| 2026-03-13–14 | Full interval library, full `SigType`, variable-delay bounds, normalization and simplification |
| 2026-03-17 | Shared `fIOTA` circular model for delay and recursion |
| 2026-03-25–27 | WebAssembly DSP backend, typed companion JSON, strict `-json`, compiler-as-WASM service, embedded libraries, and `faustwasm` integration |
| 2026-03-27 | 104-case corpus snapshot, 1,002 tests, strong front-end and C/C++ fast-lane status |
| 2026-04-03–05 | Simplification integrated into FIR preparation; variability placement; FIR CSE; recursion/delay merging |
| 2026-04-06–10 | `DelayManager`, three delay strategies, refined recursion carriers, extracted execution phases |
| 2026-04-13–24 | FAD surface, explicit seeds, De Bruijn recursion, multiple lanes, tables/waveforms, and white paper |
| Mid-April 2026 | First operational scalar milestone, roughly two months after project start |
| 2026-04-27–2026-05-21 | RAD feed-forward sweep, stateful analysis, LTI experiments, `BlockReverseAD`, TBPTT contract, and linearize-once plan |
| 2026-05-02–05 | SVG schema/device port, `-svg`, linked folding, `generateAuxFiles`, and filesystem-free `faustwasm` artifact transport |
| 2026-05-13–14 | Initial Julia backend, architecture wrapping, UI-zone fixes, precision and cast alignment |
| 2026-06-09 | Shared Box/Signal FFI context, Signal C/C++ API, headers, 269-symbol export verification |
| 2026-06-10 | AssemblyScript backend; OD/US/DS, FAD/RAD cohabitation, and vector-mode C++ analyses and staged plans |
| 2026-06-14–15 | C++ impulse protocol ported as multi-backend gates; AssemblyScript/WASM execution runners and parity fixes |
| 2026-07-07–10 | Clock-domain inference and lowering, local-time state, FAD Phase A/B, spectral/interleave examples |
| 2026-07-10–16 | `-vec`/`-vs`/`-lv`, checked `VectorPlan`, 49/93 DSPs admitted at review time, lockstep vectorization, and experimental Lean structural model |
| 2026-07-17–21 | Rust source backend, 92/92 impulse gate, runtime-contract alignment, and compiler facade API |
| 2026-07-20 | Vector module decomposition (R5–R9), byte-deterministic scalar emission, independent `clock_ad` checker, LGPL-2.1-or-later license alignment, and README marked experimental / work in progress |
| 2026-07-21 | Clean JSON diagnostic channel, `--check`, and frozen phase-wide diagnostic code table |
| 2026-07-22 | Vector coverage baseline regenerated at 97/133 certified in all 16 modes; project-local Faust libraries; published compiler Wasm artifact |
| 2026-07-23 | Native distribution library renamed `libfaust-rs` to coexist with official `libfaust`; `-ec`/`-os` porting plan written |

## Conclusion

The first phase of `faust-rs` succeeded because it combined two kinds of speed.

The first was implementation speed: AI agents could analyze large C++ areas,
generate Rust enum and matcher families, produce tests, and move between
compiler layers quickly. The hand-built FIR oscillator and the four-day
source-to-code spine — valid over a deliberately limited subset, and not yet
good code — showed how effectively intermediate contracts could decouple
unfinished work.

The second was feedback speed: golden files, C++ differentials, the FIR
verifier, the interpreter, runtime traces, and a growing DSP corpus shortened
the distance between a change and evidence about its semantics.

Neither speed would have been sufficient without architectural control. The
maintainer kept the pipeline explicit, required generic fixes, rejected
case-specific patches, and continually moved semantic ownership out of
backends and into typed, shared compiler stages.

That is the central lesson of the port so far: AI can make a compiler port move
very quickly, but only a disciplined intermediate representation, a growing
executable corpus, and firm review rules can make that velocity cumulative
rather than fragile.

The work after the first scalar milestone reinforced the same lesson at a
larger scale. Public APIs required explicit ownership contracts; WebAssembly
required compatibility with an existing runtime ecosystem; differentiation and
clock domains required new temporal semantics; and vectorization required a
checked plan rather than an emitter heuristic. In every case, the durable step
was not merely adding another feature. It was identifying the compiler layer
that owned its invariant, making that invariant representable, and turning it
into executable evidence.

## Historical sources

The main sources used for this reconstruction are:

- [Overall porting plan](../porting/faust-rust-porting-plan-en.md)
- [Initial overall assessment](../porting/faust-rust-bilan-global-en.md)
- [Initial effort assessment](../porting/faust-rust-bilan-effort-en.md)
- [Structured diagnostics model](../porting/faust-rust-diagnostics-model-en.md)
- [Enrobage porting plan](../porting/phases/phase-9-enrobage-porting-plan-en.md)
- [Structural import parity plan](../porting/parser-import-structural-cpp-parity-plan-2026-03-29-en.md)
- [FIR architecture contract](../porting/faust-rust-fir-architecture-en.md)
- [FIR module verifier plan](../porting/fir-module-verifier-plan-en.md)
- [Porting status on 2026-03-27](../porting/faust-rs-porting-status-2026-03-27-en.md)
- [FIR runtime optimization plan](../porting/fir-cse-runtime-optimizations-plan-2026-04-03-en.md)
- [Signal-to-FIR progress note](signal-to-fir-recent-progress-en.md)
- [Implementation quality assessment on 2026-05-25](../porting/faust-rs-code-quality-assessment-2026-05-25-en.md)
- [Interpreter FFI plan](../porting/faust-rust-ffi-interp-en.md)
- [Cranelift FFI parity matrix](../porting/cranelift-dsp-ffi-parity-matrix-en.md)
- [WebAssembly backend plan](../porting/wasm-backend-plan-2026-03-25-en.md)
- [WASM and strict JSON parity plan](../porting/wasm-json-parity-plan-2026-03-26-en.md)
- [SVG draw port plan](../porting/draw-svg-port-plan-2026-05-02-en.md)
- [`faustwasm` SVG auxiliary-files plan](../porting/faustwasm-svg-aux-files-plan-2026-05-03-en.md)
- [FAD and RAD synthesis](fad-rad-synthesis-en.md)
- [Julia backend plan](../porting/julia-backend-plan-2026-05-13-en.md)
- [Box/Signal API parity plan](../porting/libfaust-box-signal-api-parity-plan-2026-06-09-en.md)
- [AssemblyScript implementation journal, June 10](../porting/journal/2026-06-10.md)
- [AssemblyScript impulse validation journal, June 15](../porting/journal/2026-06-15.md)
- [Impulse-test harness port plan](../porting/impulse-tests-harness-port-plan-2026-06-14-en.md)
- [Clock-domain port plan](../porting/ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md)
- [FAD/RAD and clock-domain cohabitation](../porting/ondemand-fad-rad-cohabitation-2026-06-10-en.md)
- [Vector signal-level analysis and port plan](../porting/vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md)
- [Experimental Lean/Rust assurance plan](../porting/lean-rust-certified-porting-plan-2026-07-11-en.md)
- [Lean vector scheduling specification](../porting/vector-mode-scheduling-formal-spec.lean)
- [Rust backend implementation journal](../porting/journal/2026-07-17.md)
- [External-control and one-sample plan](../porting/external-control-one-sample-port-plan-2026-07-23-en.md)
- [Living supported-subset status](../porting/faust-rs-supported-faust-subset-en.md)
- [Daily porting journal](../porting/journal/README.md)
