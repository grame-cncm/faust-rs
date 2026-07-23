# External control and one-sample execution port plan

Date: 2026-07-23

Status: implementation plan; decision gates D1–D3 were approved by the
maintainer on 2026-07-23; no implementation has started.

Reference implementation:

- C++ Faust repository: `../faust`
- branch: `master-dev-ocpp-od-fir-2-FIR19`
- commit: `8eebea429`

## 1. Objective

Port the C++ Faust execution options below without weakening the existing
generated-code contracts:

- `-ec`, `--external-control`: remove block-rate control calculations from the
  block processing entry point and emit them in a separate `control` entry
  point;
- `-os`, `--one-sample`: emit a one-sample `frame` entry point whose input and
  output arguments are flat channel arrays rather than the block-oriented
  `compute(count, inputs, outputs)` arguments;
- support the combined `-ec -os` mode where the target language and runtime
  contract can represent it.

The implementation must keep the two dimensions orthogonal:

1. scalar versus vector scheduling;
2. block versus one-sample processing;
3. inline versus externally triggered control-rate evaluation.

`-os` must therefore not be represented as another value of the existing
scalar/vector `ComputeMode`.

## 2. C++ reference behavior

### 2.1 Global options and validation

The reference compiler stores the modes in:

- `gOneSample`, set by `-os`;
- `gExtControl`, set by `-ec`;
- `gOneSampleIO`, an internal mode used by intrinsically sample-oriented
  backends.

The relevant implementation is in:

- `compiler/global.hh`;
- `compiler/global.cpp`;
- `compiler/generator/instructions_compiler.cpp`;
- `compiler/generator/code_container.hh`;
- `compiler/generator/code_container.cpp`;
- the C, C++, and Rust code containers below
  `compiler/generator/*`.

At the pinned reference commit:

- `-os` is accepted for C, C++, D, Cmajor, FIR, and Rust (global.cpp:1903);
- `-ec` is accepted for C, C++, Cmajor, and Rust — **not** FIR (global.cpp:1910);
- `-os` is rejected with vector mode (global.cpp:1917);
- `-ec` is compatible with vector mode;
- the foreign runtime variable `count` is rejected with either option because
  neither `control` nor `frame` supplies a block count
  (instructions_compiler.cpp:1215).

Two further reference facts affect Phase 1 without changing the port's scope:

- `-ec` accepts **two** long spellings, `--ext-control` (global.cpp:1487) and
  `--external-control` (global.cpp:1693); only the latter is advertised in
  `--help`. The legacy-argument normalization should accept both for CLI
  compatibility.
- `-mem3` requires `-ec` and is C-only (global.cpp:1975, 1979). The `-mem3`
  memory manager is outside this port, but if it is ported later it inherits an
  external-control dependency; the capability model in §4.2 should leave room
  for that coupling rather than treating `-ec` as free of prerequisites.

The validation matrix is part of observable CLI behavior and must not be
reconstructed independently in several Rust match statements.

### 2.2 Instruction placement

The C++ `CodeContainer` distinguishes at least these regions:

- control declarations and statements;
- the block preamble before the sample loop;
- the sample loop;
- the post-block section.

`InstructionsCompiler::generateVariableStore` uses the classification below:

- constant values become initialized DSP state;
- block-rate values normally become stack locals in the block preamble;
- with `-ec`, block-rate values are promoted to DSP storage and their stores
  move to `control`;
- sample-rate values remain in the sample loop.

The promotion is essential. Merely moving statements into another function
would leave sample code referring to dead stack locals.

Block-rate effects follow the same ownership rule. In particular, control-rate
bargraph writes move to `control`. Soundfile-derived cached pointers, lengths,
rates, and buffers also change lifetime and placement under external control.

### 2.3 Emitted public entry points

For the C and C++ backends the observable shapes are:

| Options | `control` | `frame` | canonical `compute` |
|---|---:|---:|---|
| none | absent | absent | performs control preamble and sample loop |
| `-ec` | performs control-rate work | absent | performs sample loop |
| `-os` | absent | performs control-rate work and one sample | emitted empty |
| `-ec -os` | performs control-rate work | performs one sample | emitted empty |

The empty canonical `compute` is retained so the generated class or C API still
satisfies the ordinary DSP interface. The compiler does not make `compute`
delegate to `control` or `frame`.

`frame` has no count and receives flat arrays:

```text
frame(dsp, FAUSTFLOAT* inputs, FAUSTFLOAT* outputs)
```

Input and output accesses use channel indices directly. There is no sample
index, no per-channel block pointer alias, and no enclosing sample loop.

The host owns external-control scheduling. With `-ec`, neither initialization,
`compute`, nor `frame` implicitly invokes `control`.

### 2.4 Semantics that tests must preserve

- `-os` without `-ec` recomputes block-classified values for every frame. This
  may observe a UI change at the next sample, whereas ordinary block `compute`
  observes it at the next block.
- `-ec` leaves previously stored slow values unchanged until the host explicitly
  calls `control`.
- `-ec -os` allows one `control` call followed by any number of `frame` calls.
- lifecycle ordering remains exactly the contract in
  `porting/backend-lifecycle-contract-en.md`; `control` is not a lifecycle
  stage.

## 3. Current faust-rs gap analysis

### 3.1 CLI and compiler configuration

`crates/compiler/src/cli/args.rs` exposes neither option. Compiler and FIR
configuration currently contain the scalar/vector `ComputeMode`, but no
orthogonal execution-API or control-scheduling mode.

The legacy-argument normalization and the option JSON emitted with artifacts
must also learn the two spellings and report only modes accepted for the
selected backend.

### 3.2 Canonical FIR contract

The canonical FIR module currently assumes one processing function:

```text
compute(dsp, count, FAUSTFLOAT** inputs, FAUSTFLOAT** outputs)
```

The module verifier and `crates/codegen/src/backends/faust_api.rs` reserve and
validate the lifecycle and `compute` names, but have no typed contracts for
`control` or `frame`.

A backend-only rewrite would be unsafe because the FIR still encodes stack
lifetime, input rank, output rank, and loop ownership for block execution.
These options must be represented before language emission.

### 3.3 Scalar signal-to-FIR lowering

`ModuleSections.control_statements`
(`crates/transform/src/signal_fir/module/state.rs:54`) currently combines
statements with different execution ownership. Its own doc comment describes it
as the "`compute` preamble: channel-pointer aliases and diagnostic labels",
while `analyze_signal_sharing`
(`crates/transform/src/signal_fir/placement.rs`) routes every `Block`-rate
control value into the same list. So the section holds at least:

- slow value declarations and stores;
- input and output block-pointer aliases;
- diagnostic labels;
- per-compute resets, including block reverse-AD carry state.

The first category can move to an external `control` function. The others
cannot. The section must be split by semantics, not sliced by statement
position.

`materialize_in_bucket` currently materializes block-rate values as stack
locals (`Bucket::Control`). External control requires the C++ behavior:
DSP-state promotion plus a store in `control` and a state load at sample rate.

This promotion is **not new machinery**. `placement.rs` already implements the
same escape pattern for `konst_escapes`: a `Konst` value consumed outside its
declaring section — for example a constant reused by the block reverse-AD
reverse sweep in `compute` — is promoted from a stack-local `fConst*` to a DSP
struct field precisely so the cross-function load verifies. The `-ec` promotion
generalizes this existing, FIR-verified rule from "`Konst` descendant of a BRA
carrier" to "any `Block` value whose store now lives in `control`". Phase 2
should extend that path rather than introduce a parallel one.

Control-rate effects require a separate audit. Moving only `fSlow`/`iSlow`
expressions would miss bargraphs and soundfile-related cached state.

### 3.4 Vector lowering

The vector producer similarly combines control roots, UI stores, input aliases,
output aliases, local declarations, and the vector driver before assembling
`compute`.

`-os` must be rejected with vector mode, matching the C++ contract.

`-ec` is meaningful in vector mode, but its implementation affects the
producer/checker evidence chain. Promoted definitions and their uses must
remain explicitly attributed and verified across `control` and `compute`.
This is a finite structural artifact with silent-failure risk, so the existing
vector certificate/checker methodology must be extended rather than bypassed.
No new broad proof framework is required.

### 3.5 Block-sensitive operations

The Rust port contains operations whose semantics explicitly depend on the
block boundary and `count`. The two known reverse-mode carrier families today
are:

- `BlockReverseAD` gradient projections (index ≥ `primal_count`);
- `ReverseTimeRec` gradient projections (RAD reverse-time outputs).

`classify_reverse_time_outputs`
(`crates/transform/src/signal_fir/module/mod.rs:209`) returns the per-output mask
used by `build_module` to select a second, reverse-order sample loop for public
gradient outputs. It covers public projections from both carrier families and
should be reused as one input to the `-os` compatibility check.

It is not a sufficient oracle by itself. It deliberately stops at `SYMREC`
boundaries, and an internal `BlockReverseAD` gradient may be lowered into the
forward loop while still using block-scoped tape/carry/reset semantics. Add a
dedicated one-sample compatibility classifier over the prepared signal graph
and its lowering requirements. It must detect both carrier families wherever
their current semantics require block-owned state or reverse traversal; the
public-output mask remains the authoritative test for whether a second reverse
loop is present.

The first implementation should reject `-os` for a program containing such an
operation with a typed diagnostic. Inventing a one-sample reverse-AD meaning is
out of scope and requires a separate design decision.

The ordinary Faust foreign variable `count` must be rejected under `-ec` or
`-os`, as in C++.

## 4. Proposed internal model

### 4.1 Typed, orthogonal options

Add two enums to the compiler/FIR options, with names finalized during phase 1:

```rust
enum ControlRateMode {
    InlinePerBlock,
    External,
}

enum ProcessingApi {
    Block,
    OneSample,
}
```

Keep `ComputeMode::{Scalar, Vector}` unchanged.

Carry all three through `Compiler`, `SignalFirOptions`, cache keys, diagnostic
context, artifact metadata, and golden metadata. Defaults must reproduce
today's output byte-for-byte where practical.

### 4.2 Declarative backend capabilities

Add one backend descriptor used by CLI validation, programmatic compilation,
help/diagnostics, and tests. It should distinguish:

```text
external control: unsupported | explicit | intrinsic
one-sample API:    unsupported | explicit | intrinsic
combined mode:     unsupported | explicit | intrinsic
canonical compute: required | not applicable
```

An intrinsic mode means the backend already has a tick/control split as part of
its native contract; it does not imply that the command-line flag changes its
output.

Validation must happen before expensive parsing/lowering and must also be
enforced by programmatic entry points, not only by Clap.

### 4.3 Execution-owned FIR sections

Replace the overloaded control statement list with explicit ownership, for
example:

- externalizable control computations and effects;
- block-compute preamble, including block I/O aliases;
- sample body or sample loop;
- post-compute effects;
- lifecycle functions.

The exact Rust type may be an execution-section enum or separate typed
collections. It must prevent block I/O aliases and per-block resets from being
accidentally emitted in `control`.

The scalar and vector producers should share the execution contract and state
promotion helper, while retaining their own scheduling logic.

### 4.4 Promotion invariant

For every value or effect moved from a block preamble to `control`:

1. its result lives in DSP-owned storage;
2. `control` is the only runtime writer, except lifecycle initialization where
   required;
3. `compute` and/or `frame` read that storage;
4. dependency order within `control` is deterministic;
5. no control value remains as an out-of-scope stack reference;
6. side effects are emitted exactly once per explicit `control` invocation.

This is an `adapted` representation relative to the C++ container layout and
requires a structural non-regression test plus documentation in the relevant
phase record or daily journal.

Invariants 1, 2, and 5 are exactly the property the existing `konst_escapes`
promotion already enforces for BRA-escaping constants (§3.3). Reusing that path
means the "no out-of-scope stack reference" guarantee is checked by the same FIR
verifier rule that already rejects a `LoadVar(Stack, "fConst*")` outside its
declaring function, rather than by a new bespoke check.

### 4.5 FIR functions

Extend the canonical FIR API with typed optional functions:

```text
control(dsp)
frame(dsp, FAUSTFLOAT* inputs, FAUSTFLOAT* outputs)
```

The module verifier must validate:

- reserved-name uniqueness;
- exact arguments and result types;
- flat frame input/output rank;
- module input/output arity;
- required DSP-state accesses;
- absence of `count` from both functions;
- absence of a sample loop from `frame`;
- empty canonical `compute` in one-sample mode;
- lifecycle ordering unchanged.

The canonical FIR should expose the selected execution shape. This makes
`-lang fir` useful for structural debugging and allows backends to consume one
verified contract instead of re-deriving it.

### 4.6 One-sample lowering

One-sample mode must be selected while assembling the FIR module:

- emit direct channel input loads and output stores;
- emit exactly one sample body;
- omit the `count` argument and sample loop from `frame`;
- retain state updates and post-sample work;
- include slow computations in `frame` only when control mode is inline;
- emit an empty canonical `compute`.

A late text-emitter transformation is not acceptable because it cannot safely
repair pointer rank, local lifetime, effect ownership, or loop-dependent state.

## 5. Backend support matrix

The table distinguishes the initial port from technically possible future
extensions.

| faust-rs backend | `-ec` | `-os` | combination | Initial decision |
|---|---:|---:|---:|---|
| C | yes | yes | yes | Implement, differential against C++ |
| C++ | yes | yes | yes | Implement, differential against C++ |
| Rust | yes | yes | yes | Implement while preserving `FaustDsp::compute` |
| FIR text | yes* | yes | yes* | Emit and verify the canonical execution shape |
| Interp/FBC | no | no | no | Reject initially; versioned runtime/API work needed |
| Cranelift JIT | no | no | no | Reject until multi-entry lifecycle ABI is complete |
| WebAssembly/WAST | no | no | no | Reject; current WebAudio/block ABI has no contract |
| AssemblyScript | no | no | no | Reject in the initial port; planned one-sample follow-up (§5.7) |
| Julia | no | no | no | Reject; current `compute!` contract is block-oriented |

`*` The pinned C++ CLI accepts `-os -lang fir` but rejects
`-ec -lang fir`. The approved faust-rs policy accepts external control for the
Rust FIR diagnostic backend because FIR is the verified representation that
every supporting source backend must consume. This is an intentional `adapted`
extension, not a C++ CLI parity claim.

### 5.1 C and C++

These are the reference contracts:

- add language-specific `control` and `frame` signatures;
- retain the empty ordinary `compute` in one-sample mode;
- make methods/public functions discoverable by existing architecture files;
- do not insert implicit control calls;
- compile generated output with zero, one, and multiple input/output channels.

### 5.2 Rust

The pinned C++ Faust Rust backend emits both entry points, so the Rust port
should support the complete matrix.

The existing external `FaustDsp` block contract must remain source-compatible.
The initial design should:

- keep the canonical trait `compute` method;
- emit inherent/public `control` and `frame` methods on the generated DSP type;
- make `compute` empty in one-sample mode;
- do not add methods to the host-supplied `FaustDsp` trait as part of this port.

Contract-affecting changes must be compiled inside representative
`faust2jackrust -source` and `faust2portaudiorust -source` projects as required
by `AGENTS.md`.

### 5.3 FIR text

FIR has no host ABI, so representing both dimensions is meaningful and greatly
improves verification. The public CLI accepts `-ec -lang fir` as the approved
diagnostic extension, and documentation must distinguish it from strict C++
CLI parity.

### 5.4 Interpreter

The FBC runtime already distinguishes a control block from a DSP block
internally, but `try_compute` currently executes both as one public block
operation. Exposing `-ec` would change scheduling and serialization/runtime
compatibility; `-os` would require a public frame API and flat I/O contract.

Do not partially enable the flags. A later interpreter project may add:

- versioned FBC execution metadata;
- explicit `try_control`;
- explicit `try_frame`;
- old-file compatibility rules;
- optimized/unoptimized execution parity.

### 5.5 Cranelift

The current backend does not yet provide the mature multi-entry public ABI
needed to export and safely discover `control`, `frame`, and canonical
`compute`. Enabling either option now could silently fall back to a stub.
Defer until lifecycle conformance and multi-symbol lookup are complete.

### 5.6 WebAssembly/WAST

The current contract is coupled to a block `compute`, linear-memory pointer
tables, and WebAudio/runtime glue. Additional exports are technically possible
but would define a new public ABI rather than port the reference behavior.
Reject both flags until an architecture and versioning proposal exists.

### 5.7 AssemblyScript and Julia

Both current generated-source contracts and their integration tests are
block-oriented. C++ Faust does not accept these flag/backend pairs. Reject them
with a capability diagnostic rather than emitting functions unused by the
host.

For Julia this is the end state until a concrete host contract exists.

For AssemblyScript the rejection is an initial-port sequencing decision, not
an end state. A one-sample AssemblyScript target is a planned follow-up once
C, C++, and Rust land, for these reasons:

- A concrete downstream host already consumes one-sample AssemblyScript
  today: the wasm-music `MidiVoice` engine (`noteon`/`noteoff`/`nextframe`
  per voice, per-sample mixing across voices). It currently obtains the
  control/sample split by post-processing the block `-lang asc` output
  (stripping the `count` loop and separating `fSlow`/`iSlow` control code
  from the per-sample body) — exactly the fragile late text transformation
  §4.6 rules out. A native target removes that layer.
- The useful shape for such hosts is the combined `-ec -os` mode: `-os`
  without `-ec` recomputes control values every frame (§2.4), which
  per-sample voice engines cannot afford. Plans for the AssemblyScript
  target should treat `-ec -os` as the primary supported combination.
- The block `compute` contract must remain the default; the one-sample
  target is additive (a second execution shape under the §4.2 capability
  model, `explicit` for one-sample and external control), not a
  replacement.

Unlike C/C++/Rust, C++ Faust has no `-os` AssemblyScript reference, so this
is a new public contract decision (an `adapted` extension in the API-mapping
vocabulary), not a parity port: the `frame`/`control` AssemblyScript
signatures and their host-integration story must be designed and approved
explicitly. The §4.2 capability descriptor and the Phase 2 FIR execution
contract already provide everything such a backend must consume; no
additional FIR work is expected beyond what C/C++/Rust require.

Until that follow-up is approved and implemented, both flags stay rejected
for AssemblyScript with the same capability diagnostic as Julia.

### 5.8 Future/scaffolded backends

When these backends become active, initialize their descriptors from the
reference behavior:

| Backend | Reference interpretation |
|---|---|
| Cmajor | intrinsically one-sample with separated control; accepted flags are output-invariant compatibility aliases |
| Codebox | intrinsically one-sample/control-separated; keep explicit options rejected unless its CLI contract is revised |
| D | supports one-sample only, using its existing D-specific public signature; external control rejected |
| JAX | intrinsically sample-oriented internally, but explicit flags remain unsupported |
| C#, JSFX, LLVM, SDF3, VHDL | unsupported until a backend-specific public contract and C++ parity case exist |
| Java, legacy OCPP | outside the frozen Rust port scope |

For an intrinsic backend, add output-invariance tests showing that accepted
compatibility flags do not duplicate control execution or wrap an existing
tick loop in another frame layer.

## 6. Implementation phases

### Phase 0 — scope confirmation and baselines

Pass criteria:

- confirm the production path:
  `parse -> boxes -> eval -> propagate -> normalize -> type/interval ->
  transform -> fir -> backend`;
- record the reference command lines and generated C/C++/Rust artifacts for all
  four option combinations;
- add compact differential DSP cases for slow values, state, UI effects,
  soundfiles, and multiple channels;
- confirm that this work adds explicit compiler context and no new global
  equivalent to `gOneSample`/`gExtControl`;
- record the approved D1 and D2 policies in diagnostics and API tests;
- record the approved unchanged-`FaustDsp`/inherent-method Rust contract in
  emitter API tests before implementation.

No large FIR representation change starts before these checks are recorded in
the relevant phase document or journal entry.

### Phase 1 — options and capability validation

Deliverables:

- typed orthogonal execution options;
- Clap short and long flags;
- legacy-argument normalization;
- one declarative backend capability table;
- early validation for CLI and library APIs;
- `-os` plus vector rejection;
- `count` and block-sensitive-operation diagnostics;
- cache/metadata/golden option identity.

Pass criteria:

- default compilation is unchanged;
- every active backend has an explicit tested capability result;
- invalid combinations fail before code generation with stable diagnostics;
- no backend silently ignores a non-intrinsic flag.

### Phase 2 — FIR execution contract

Deliverables:

- execution-owned module sections;
- typed `control` and `frame` functions;
- block-value/effect state promotion;
- direct one-sample I/O lowering;
- empty canonical `compute` generation;
- verifier and Faust API contract updates.

Pass criteria:

- structural tests cover all four execution shapes;
- no stack local crosses a function boundary;
- `frame` contains no block loop or `count`;
- external `control` contains no block I/O alias or per-compute reset;
- lifecycle conformance remains green.

### Phase 3 — scalar C, C++, and FIR

Deliverables:

- C and C++ signatures and function emission;
- FIR text emission for the approved capability policy;
- golden structural output for the four combinations.

Pass criteria:

- generated C and C++ compile with strict warnings;
- functional outputs match the pinned C++ compiler on the differential corpus;
- UI/soundfile effect-count tests pass;
- empty `compute` and explicit host scheduling are verified.

### Phase 4 — Rust backend

Deliverables:

- public inherent `control` and `frame`;
- retained `FaustDsp::compute` compatibility;
- source and runtime tests for each combination;
- external architecture-project builds.

Pass criteria:

- existing block consumers compile unchanged;
- generated one-sample source compiles with host-supplied
  `F32`/`F64`/`FaustFloat` and `ParamIndex`;
- representative architecture projects build;
- runtime outputs match C++ Rust output behavior.

### Phase 5 — vector external control

Deliverables:

- vector control-root promotion;
- explicit separation of vector I/O/driver preamble from external control;
- producer/checker certificate extensions;
- scalar/vector external-control differential tests.

Pass criteria:

- `-ec -vec` matches reference behavior;
- certificate corruption tests reject missing, duplicated, or misattributed
  promoted control events;
- ordinary vector output is unchanged;
- `-os -vec` remains a stable early error.

### Phase 6 — hardening and documentation

Deliverables:

- full golden coverage and metadata;
- user-facing CLI/backend capability documentation;
- relevant public API mapping entries (`1:1`, `adapted`, or `deferred`);
- daily journal entry and, for a multi-step session, `porting/HANDOFF.md`;
- explicit deferred-backend issue list.

Pass criteria:

- `cargo fmt --all`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo test --workspace --all-targets`;
- `cargo run -p xtask -- golden-check`;
- supported C++ differential suite green on the pinned compiler;
- Linux/macOS/Windows-safe tests and paths.

## 7. Required test matrix

### 7.1 Structural FIR tests

For each of none, `-ec`, `-os`, and `-ec -os`, assert:

- function presence and exact signatures;
- canonical `compute` body ownership;
- control value storage class;
- absence of cross-function stack references;
- direct versus block I/O shape;
- deterministic statement ordering;
- lifecycle function ordering.

### 7.2 Runtime semantic tests

Use compact inline Faust sources:

- slider multiplied by input, to distinguish control sampling;
- stateful recursion/oscillator, to validate frame-to-frame state;
- control-rate and sample-rate bargraphs, to count effects;
- multi-input/multi-output routing;
- zero-input and zero-output programs;
- soundfile operations with test-local fixtures;
- a program using `count`, expecting rejection;
- `BlockReverseAD` and `ReverseTimeRec`, including public reverse outputs and
  internal BRA use, expecting the approved `-os` policy.

Key schedules:

1. ordinary `compute(N)` with fixed controls;
2. `control(); compute(N)` under `-ec`;
3. `frame()` repeated `N` times under `-os`;
4. `control(); frame()` repeated `N` times under `-ec -os`;
5. change a UI value without calling `control`, then call it and verify the
   boundary at which the change becomes visible.

Block/frame equivalence assertions must use fixed controls unless the test is
specifically checking the expected per-frame UI sampling difference.

### 7.3 Differential and contract tests

- Compare behavior with the C++ compiler pinned above for C, C++, and Rust.
- Check important generated signatures and ownership structurally; avoid
  brittle full-file comparisons outside the golden workflow.
- Build generated code against actual architecture contracts.
- Test `float` and `double`, optimization levels used by the backend, and both
  scalar and vector external-control paths.
- Include negative capability tests for every active unsupported backend.

## 8. Approved compatibility decisions

### D1 — public FIR capability

Approved on 2026-07-23: accept `-ec` for Rust FIR as an explicitly documented
diagnostic extension.

Impact: CLI parity versus observability of the verified intermediate form. It
does not change a deployed runtime ABI. Tests and documentation must state that
this is an intentional faust-rs extension rather than C++ CLI parity.

### D2 — one-sample block-sensitive operations

Approved on 2026-07-23: reject `-os` when the FIR requires block count or block
boundaries. The known reverse-mode carrier families in this class are
`BlockReverseAD` and `ReverseTimeRec`; both are in scope from the start.
Detection must use a dedicated one-sample compatibility classifier.
`classify_reverse_time_outputs` (§3.5) is reused for public reverse-loop
requirements, but cannot be the sole oracle because it intentionally excludes
some internal BRA uses.

Impact: this fixes the unsupported-feature policy without inventing new
mathematical semantics. Any future persistent one-sample meaning requires a
separate design and explicit approval.

### D3 — Rust trait surface

Approved on 2026-07-23: preserve the required block `FaustDsp` trait unchanged
and add generated `control` and `frame` as public inherent methods.

Impact: existing Rust architecture projects retain source compatibility. Any
future defaulted or versioned trait extension is outside this port.

## 9. Definition of done

The port is complete when:

- the option matrix is explicit and enforced for every active backend;
- C, C++, and Rust implement all four shapes;
- FIR represents and verifies the selected execution contract;
- vector external control is certificate-checked;
- unsupported backends fail clearly and never ignore flags silently;
- lifecycle and existing generated-code contracts remain compatible;
- differential, golden, architecture-build, and workspace quality gates pass;
- adapted and deferred API mappings are documented.
