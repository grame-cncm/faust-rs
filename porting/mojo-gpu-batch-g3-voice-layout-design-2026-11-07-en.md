# G3: per-voice state as a structure of arrays. Design

Date: 2026-11-07

Status: **G3 done 2026-11-07: decisions §6, outcomes §7 to §10. The Mojo
batch mode landed by the user's decision, its pass criterion met on five
cells of six (§10).**
Parent plan: `porting/mojo-gpu-batch-of-voices-plan-2026-11-07-en.md`
(G3 in §3, the measurements that call for it in §8).

## 1. Why

G2 measured, on the Apple M5 through Metal, that programs with a large
state lose on the GPU and degrade faster than the voice count grows:
`freeverb` (150 KB per voice) takes 6.5 ms per 256-frame block at 1,024
voices and 102 ms at 4,096. In the array-of-structures layout of G1 and G2,
voice v owns one whole `mydsp`; thread v walks its own structure, so the 64
threads of a SIMD group read 64 addresses one structure apart, and the
state, 600 MB at 4,096 voices, streams through the caches without reuse.

In a structure of arrays, element k of a state field is stored for all
voices side by side, at `k * N + v`. Threads v and v+1 executing the same
access read neighbouring words; the 64 reads of a SIMD group are one
contiguous segment.

### 1.1 Measured before designing

A synthetic kernel with no compiler change (`freeverb`-like: per voice, 16
feedback combs on delay lines of 2,048 floats, 128 KB, indexed by a shared
counter masked to the line size as the generated code does), the two
layouts on the same device, 256-frame blocks, median of 40 blocks after 20
warm-up blocks, kernel and synchronisation only:

| voices | state | one structure per voice | structure of arrays | gain |
|---|---|---|---|---|
| 256 | 33 MB | 1.2 ms | 1.1 ms | ×1.1 |
| 1,024 | 134 MB | 1.7 ms | 1.2 ms | ×1.4 |
| 4,096 | 536 MB | 47.6 ms | 1.5 ms | ×31 |
| 16,384 | 2.1 GB | 198 ms | 5.1 ms | ×39 |

The structure-per-voice column reproduces the collapse of the real
`freeverb` (102 ms at 4,096). The structure of arrays stays nearly flat,
and at 4,096 voices is under the 7.4 ms the CPU takes for the real program
on four threads. This clears the ×2 threshold G3 set (§3 of the plan) on a
synthetic kernel; the real programs are measured in G3.5 (§5).

## 2. What the FIR offers (read on 2026-11-07)

Repository-relative paths; line numbers as of `8a104629`.

- **Fields.** Instance state is `FirMatch::DeclareVar { name, typ, access:
  Struct, init: None }` in the module's `dsp_struct` section
  (`crates/fir/src/matcher.rs:123`, `:279-294`): scalars (`fRec0`,
  `fHslider0`, `fSampleRate`, `fIOTA`, `fConst*`) and fixed arrays
  `FirType::Array(T, n)` (delay lines `fVec*`, recursion buffers `fRec*` of
  2). Initial values are `StoreVar`s in `instanceConstants`,
  `instanceResetUserInterface` and `instanceClear`
  (`crates/transform/src/signal_fir/module/state.rs:504-520`).
- **Shared state.** Read-only generated tables and waveforms are `Static`
  declarations in `static_decls` (`crates/transform/src/signal_fir/module/tables.rs:291-345`),
  written once by `staticInit`. Foreign variables are `Global`.
- **Accesses.** Every access names its field: `LoadVar`, `StoreVar` for a
  scalar, `LoadTable`, `StoreTable` with an index expression for an array
  element (`matcher.rs:65-172`), built with `FirBuilder` (`builder.rs:181-502`).
  Index arithmetic is ordinary `binop` nodes. The scalar lowering emits no
  `ShiftArrayVar`, and a struct `TeeVar` is already refused by the Mojo
  emitter.
- **User interface.** `AddButton`, `AddSlider`, `AddBargraph` carry their
  zone as a field *name* (`var: String`, `matcher.rs:223-240`), not as an
  address expression.
- **Method signatures.** Canonical methods are `DeclareFun` with
  `named_args`; `codegen::backends::faust_api::validate_canonical_dsp_api_signature`
  requires the exact canonical arguments (`crates/codegen/src/backends/faust_api.rs:36-116`),
  so an extra argument is refused today (FRS-CGEN-MOJO-0002).
- **Precedent for a checked FIR-to-FIR pass.** `fir::schedule`
  (`crates/fir/src/schedule/mod.rs:112-170`): imports into a fresh store,
  rewrites, and keeps each stage only if an independent checker accepts it
  (`schedule/check.rs`, `check_reuse.rs`), with mutation tests
  (`schedule/tests.rs:260+`).
- **Where Mojo gets its module.** `generate_mojo_module` calls
  `prepare_flat_module(.., MergedStructFields, ..)`
  (`crates/codegen/src/backends/mojo/mod.rs:194-199`): table generators are
  flattened into `{Sub}_{name}` struct fields, so the pass sees one flat
  module.

## 3. The design

### 3.1 The layout

N voices, a run-time count. For every scalar element type T used by
instance state (`FaustFloat`, `Int32`, and `Float64`/`Int64` when present),
one *arena*: a buffer of `S_T * N` elements, where `S_T` is the number of
type-T state elements of one voice. The fields of type T are laid out in
declaration order: field f gets a base `b_f` and a length `c_f` (1 for a
scalar, n for `Array(T, n)`), with `b_{f+1} = b_f + c_f`. Element k of field f
of voice v is

    arena_T[(b_f + k) * N + v]

so all voices of one element are contiguous. Static state (the
`static_decls` tables) is left as it is, shared by all voices: one copy of
`osc`'s 256 KB table instead of one per voice, which is the fix G2 named for
`osc`.

### 3.2 The pass

`fir::batch::layout_voices(src: &FirStore, module) -> Result<(FirStore,
FirId, VoiceLayout), BatchError>`, in `crates/fir/src/batch/`, on the
flattened module, after `prepare_flat_module`.

1. Collect the `Struct` declarations of `dsp_struct`; compute `VoiceLayout`
   (per field: arena type, base, length; per type: `S_T`).
2. Rebuild `dsp_struct` as: one `Struct` pointer field per arena
   (`fState<T>: Ptr(T)`), and `fVoices: Int32` (N). Nothing else.
3. Rewrite every access to an instance field, in every function body:
   - `LoadVar{f, Struct}` → `LoadTable{fState<T>, Struct, b_f * N + voice}`;
   - `StoreVar{f, Struct, x}` → `StoreTable{fState<T>, Struct, b_f * N + voice, x}`;
   - `LoadTable{f, Struct, i}` → `LoadTable{fState<T>, Struct, (b_f + i) * N + voice}`;
   - `StoreTable{f, Struct, i, x}` → the same with the value;
   where `N` is `LoadVar{fVoices, Struct}` and `voice` is
   `LoadVar{voice, FunArgs}`. Static and global accesses are untouched.
4. Add the argument `voice: Int32` to `instanceConstants`,
   `instanceResetUserInterface`, `instanceClear` and `compute`; `staticInit`
   keeps its signature (shared state only).
5. Refuse (`BatchError`) what the pass does not handle rather than pass it
   through: a `LoadVarAddress`, `TeeVar` or `ShiftArrayVar` on an instance
   field, a remaining sub-module, a soundfile, a field touched by both
   `staticInit` and the instance lifecycle. A `-vec` module is not refused
   (nothing in its FIR marks it reliably); it is untested until G3.4.

The output is a backend-neutral FIR module: the arithmetic of the layout is
FIR, visible to every checker and backend, and the emitter only renders
pointers and indices, which it already does (`mojo/mod.rs:1313-1331`).

### 3.3 The independent checker

`fir::batch::check::check_voice_layout(src, src_module, dst, dst_module) ->
Result<(), LayoutViolation>`, written apart from the producer and sharing no
code with it beyond the FIR matcher:

- recompute the layout from the *source* declarations, by its own walk;
- check it is injective and in bounds: field ranges disjoint and inside
  `[0, S_T)` for each type, every element type assigned to exactly one
  arena;
- walk source and output bodies in lockstep: every non-access node
  identical; every instance access in the source matched by an access in
  the output on its type's arena with index `(b_f + i) * N + voice`
  (scalar: `b_f * N + voice`), with `i` the source index unchanged; every
  static or global access unchanged;
- check no `Struct` access to an instance field remains, and that only the
  four lifecycle functions gained `voice`.

Rejecting mutations, each a test that must fail the checker: two fields
given overlapping bases; a field's length one short; the stride dropped
(`b_f + i + voice`); `voice` dropped; a static table moved into an arena; one
access left on its old field; `voice` added to `staticInit`.

### 3.4 Differential test, without a GPU

The pass is backend-neutral, so its semantics can be checked on the CPU in
CI: for a corpus subset, run the batch module for N voices (each voice its
own inputs and control values) and N independent runs of the scalar module,
and require bit-identical outputs voice by voice. Where it runs is D4.

### 3.5 The Mojo side

- `MojoOptions` gains a batch flag; when set, the emitter runs the pass and
  renders the batch contract: the struct holds the arena pointers,
  `f_voices` and the shared static fields; methods take the voice,
  `compute(mut dsp, var voice: S32, var count: S32, var inputs: ImmStreams,
  var outputs: MutStreams)`; a `state_elements(T)` query per arena (the host
  allocates `S_T * N`); a `FaustDspBatch` trait in the test runtime.
- Lifecycle, adapted from `porting/backend-lifecycle-contract-en.md`:
  `class_init` once for all voices; `instance_init(voice) =
  instance_constants(voice) -> instance_reset_user_interface(voice) ->
  instance_clear(voice)`, never `class_init`; a batch conformance test
  checks it as the scalar one does.
- The device harness passes `global_idx.x` as the voice; the struct itself
  is read-only on the device (only the arenas are written).
- Without the flag, Mojo output is unchanged, checked by the
  codegen-identity differential over the corpus.

## 4. Consequences

- A new, Rust-only generated contract (the batch methods and trait) and an
  option to request it: a registry entry
  (`porting/faust-rs-vs-faust-cpp-differences-en.md`) when it lands.
- No cost for the default pipeline: the pass runs only when asked, so the
  compile-budget gate is unaffected; still run before committing the
  emitter change.
- The arenas make every state access an indexed load (`(b + i) * N + v`):
  on the GPU this is what buys coalescing; on the CPU, the same layout puts
  N voices in consecutive lanes, which is the CPU widening the September
  analysis (§3.3) named. Measured, not assumed, in G3.5.
- Each voice still has its own copy of `fSampleRate`, `fConst*` and `fIOTA`:
  voices stay independent instances (a voice can be reset alone, as
  polyphony needs). Sharing the constants is a later optimisation.

## 5. Phases

Each with its deliverable and pass criterion fixed before it starts.

- **G3.0 — measurement (done, §1.1).** Pass: gain ≥ ×2 at 4,096 voices on
  the synthetic kernel. Met: ×31.
- **G3.1 — decisions (§6).** Answers recorded here before any code.
- **G3.2 — producer.** `fir::batch::layout_voices` and `VoiceLayout`. Pass:
  on every corpus program the scalar lowering compiles, either a batch
  module or a stated `BatchError`; unit tests on a hand-built module.
- **G3.3 — checker and mutations.** Pass: the checker accepts the producer
  on the whole corpus and rejects every mutation of §3.3.
- **G3.4 — CPU differential (§3.4).** Pass: bit-identical per voice on the
  corpus subset, N = 1, 3 and 64, voices with distinct inputs and controls.
- **G3.5 — Mojo batch emission and measurement.** The emitter mode, a
  `make mojo-gpu-batch` lane (the G1 checks, the CPU oracle being the
  scalar program), and `make mojo-gpu-bench` with the batch layout. Pass:
  `freeverb`, `zita_rev1` and `karplus32` at 1,024 and 4,096 voices at least
  ×2 faster than the array-of-structures layout of §8; the plan records
  whether the device then beats the four performance cores. If the gain is
  under ×2, the emitter mode does not land and the result is recorded.

## 6. Decisions (taken 2026-11-07)

Answered by the user on 2026-11-07, each the recommended option:

- **D1: the voice count is fixed at run time** (arenas sized by the host).
- **D2: controls are per voice**, with the table of control bases.
- **D3: an emitter option only** (`MojoOptions`), no CLI surface until G4;
  no registry entry for a CLI option, one for the generated batch contract
  when G3.5 lands.
- **D4: the differential test runs in the Rust suite through the
  interpreter**, which therefore accepts the batch signatures (G3.4).

The options as they were put:

- **D1 — Voice count: at run time or at compile time.** Run time
  (recommended): one build serves any N, the struct stays small and
  copyable, the arenas are buffers the host sizes. Compile time (`N` fixed
  by an option, arrays `Array(T, c_f * N)` in the struct): no pointer, but
  one build per voice count, and a struct of hundreds of MB that Mojo
  builds on the stack in `mydsp()`.
- **D2 — Controls: per voice or shared.** Per voice (recommended for
  polyphony and training batches): control fields go into the arenas like
  any state, and since the UI nodes name their zone, the batch module also
  carries, in UI order, the arena base of every control (a `Static` table
  the pass declares), so the host sets control i of voice v at
  `arena[(b_i) * N + v]`; `build_user_interface` describes labels and ranges
  once. Shared: control fields stay ordinary struct fields read by all
  voices, the UI is unchanged; simpler, but every voice plays the same
  settings.
- **D3 — The option.** A faust-rs-only CLI option for `-lang mojo` (name to
  choose, for instance `--batch-voices`), which makes the mode usable
  outside the tests and needs a registry entry; or an emitter option only
  (`MojoOptions`), reached by the test lanes through the Rust API, with no
  CLI surface until G4.
- **D4 — Where the differential test of §3.4 runs.** In the Rust test
  suite through the interpreter or Cranelift backend (recommended: in CI, on
  every platform, no Mojo needed), which needs those backends to accept the
  batch signatures; or only in the opt-in Mojo CPU lane.

## 7. G3.2 outcome (2026-11-07)

`crates/fir/src/batch/` (`mod.rs`, `tests.rs`): `layout_voices`, `VoiceLayout`,
`Placement`, `Arena`, `BatchError`, `arena_name`, and the names it adds
(`VOICES_FIELD`, `VOICE_ARG`, `CONTROLS_TABLE`, `INSTANCE_FUNCTIONS`). Built
like `fir::schedule`: import into a fresh store, rewrite, rebuild the
module. The rewrite is generic: a node that is not one of the four access
kinds is rebuilt from its arena kind with its children rewritten, so every
FIR construct, present or future, is traversed without a per-variant arm.
Not wired into any backend: no output changes.

Choices made while implementing, within the design:

- *Shared `Struct` fields.* Under `MergedStructFields`, an inlined table
  generator keeps its working state in `Struct` fields written by
  `staticInit`. A field touched only by `staticInit` stays shared; one
  touched by both `staticInit` and the instance lifecycle is refused. A
  literal `Struct` table is shared and must not be written.
- *Explicit store coercion.* The producer stores a `Float32` constant into a
  `FaustFloat` field with a `StoreVar`, which the FIR verifier does not
  type; as a `StoreTable` on the arena it does (FIR-T02). The pass casts the
  stored value to the arena's element type when the two differ, making the
  coercion every backend applies on a store explicit.
- *Indices are `Int32`.* A host must keep `elements * N` under 2^31:
  `karplus32` (120,419 `Float32` elements per voice) allows about 17,800
  voices.

Unit tests (14): the layout of a hand-built module with every kind of state
(bases, lengths, arenas, shared fields, controls), the rebuilt `dsp_struct`
and control table, the voice argument on the four instance functions and
nowhere else, no access left to a per-voice field, the exact index shape
for an element and a scalar, an explicit cast on a mistyped store,
determinism, the FIR verifier accepting the output, and one refusal per
`BatchError` the module can trigger.

**Corpus, the pass criterion.** The 133 programs of
`tests/impulse-tests/dsp`, lowered by the transform lane and flattened with
`MergedStructFields` (as `-lang mojo` does), through a temporary driver not
committed (the corpus needs the local Faust libraries, which committed
tests must not): 132 laid out, 1 refused with its reason (`sound`,
soundfile), none failing. The FIR verifier reports no new error on any of
them (the `sin` errors some programs carry exist before the pass). 36
programs keep shared fields (generator state); every program has an
`Int32` arena, 102 a `Float32` one, 89 a `FaustFloat` one (their controls).
`osc` drops from 262,180 bytes per voice in G2 to 3 `Float32`, 2
`FaustFloat` and 1 `Int32` elements, its table shared; `freeverb` has
37,440 `Float32` elements per voice, `zita_rev1` 119,905.

## 8. G3.3 outcome (2026-11-07)

`crates/fir/src/batch/check.rs`: `check_voice_layout(src, src_module, dst,
dst_module) -> Result<(), LayoutViolation>`, with a named rule per
violation. It shares no code with the producer beyond the FIR matcher and
restates the contract (the added names, the instance functions) itself.

- *Classification* is read from the output and checked against the source:
  a source `Struct` variable absent from the output structure is per-voice,
  one present must be declared identically and is shared. The semantics is
  then checked on the source: no instance function touches a shared field,
  no other function (`staticInit` included) touches a per-voice one, every
  control is per-voice, a literal table stays shared, and the output adds
  exactly `fVoices` and one `Ptr(T)` arena per per-voice element type.
- *Bases* are recomputed from the source declarations (per element type, in
  declaration order, each after the previous length), so the checker never
  reads the producer's `VoiceLayout`.
- *Bodies* are walked in lockstep across the two stores (tags compared by
  name, memoised by node pair): a per-voice access must sit on its arena at
  `(base + i) * fVoices + voice`, `i` the source index, a store cast to the
  arena type exactly when its value's type differs; every other node must be
  identical. Signatures: the four instance functions gain `voice` after
  `dsp`; any other function must keep its own.
- *Controls*: `fVoiceControls` holds the bases in user-interface order,
  declared only when there is a control.

`layout_voices` now returns its output only once the checker accepts it
(fail closed, `BatchError::Rejected`), as `fir::schedule` does. The producer
without its checker is `layout_voices_with(.., Mutation)`, crate-private,
whose `Mutation` injects one defect for the rejecting tests.

Tests (18 in the module, 4 new): the checker accepts the producer, with and
without a cast store; it rejects each of eight mutations under the expected
rule, the seven of §3.3 and a dropped cast (overlapping bases, a length one
short, the stride dropped, the voice dropped: `index`; a shared field moved
into an arena: `class use`; an access left on its field: `residual`; the voice
added to `staticInit`: `signature`; an uncast store: `store`); it rejects an
output checked against another source, and the unchanged source.

**Corpus, the pass criterion.** Through a temporary driver (deleted): the
checker accepts the producer on all 132 programs the producer lays out
(`sound` refused as before), none rejected. Producer and checker together
take at most 9 ms per program (`spectral_level`), in release mode.

## 9. G3.4 outcome (2026-11-07)

**The interpreter runs batch modules** (D4), through a separate entry point
so that the 19 callers that build `InterpOptions` literally are untouched:
`codegen::backends::interp::generate_interp_batch_module(store, module,
options, &InterpBatch)`, `InterpBatch::from_layout(voices, &VoiceLayout)`.

- The interpreter has one int heap and one real heap and no indirection, so
  each arena (`Ptr(T)` field) gets a region of `elements * N` slots on the
  heap of its element type; the layout's indices stay inside it.
- The `voice` argument is a reserved int slot, as `count` already was;
  `fVoices` is written by the instance at construction (`FbcBatchLayout` on
  the factory, not written to `.fbc`).
- Each control resolves to its voice-0 slot (`base * N` in its arena), so the
  user-interface offsets keep working and a host adds the voice.
- The instance gains `instance_init_voice(sample_rate, voice)`,
  `compute_voice(voice, count, inputs, outputs)`, `set_voice_zone` and
  `get_voice_zone`; `class_init` runs once for all voices.
- The ordinary path is unchanged: the new storage rule applies only to the
  arenas named by a batch, and the `voice` slot only to a `voice` argument,
  which no ordinary module reads.

**The differential** (`crates/compiler/tests/batch_voices_interp.rs`, in CI):
seven inline programs without libraries (a recursion behind a slider, a
variable delay with its circular line and counter, an integer recursion
behind a slider, a wrapped-phase oscillator, a generated table whose
generator state stays shared, two channels, a button), each laid out and run
for N = 1, 3 and 64 voices against N independent scalar instances, at
interpreter optimisation levels 0 and max. Every voice has its own inputs
(an impulse scaled per voice plus a sine of voice-dependent frequency) and
its own control values, and the test asserts that voices 0 and 1 differ.
**Pass: every voice bit-identical to its scalar instance, on every program,
voice count and optimisation level.** Sanity: with `set_voice_zone`
temporarily writing voice 0's control for every voice, the test fails at
once (`onepole`, 3 voices, voice 1 at frame 1); restored, it passes.

No registry entry yet: the layout is reachable only through the Rust API
of the tests; the entry comes with the generated Mojo batch contract
(G3.5), as D3 says.

**Defect found 2026-11-08: the sample rate.** None of the seven programs
read the sample rate, and neither did the three corpus voices added later
(they hard-code 48000). `gpu_choir_partial.dsp` does, and the differential
failed on it. The layout moves `fSampleRate` into the int arena, so the
module has no `fSampleRate` field. The interpreter compiles the `sample_rate`
argument to a slot it reserves when no rate field exists, but the factory
resolved the slot the host writes from `fSamplingFreq` or `fSampleRate`
only, and reserved a second one. Every voice read 0 and ran at a rate of 1
(`max(1, 0)`). Voice 0 of a test can hide it: its control sits at the bottom
of its range, which for a gain is 0. The factory now falls back to the
`sample_rate` slot (`crates/codegen/src/backends/interp/mod.rs`, step 6). A
scalar module always has `fSampleRate`, so it is unaffected. The differential
gains `samplerate`, a program reading the rate in `instanceConstants` with a
control range that starts above 0, and the corpus voice; both fail without
the fix. The generated Mojo batch code was never affected, since it stores
the `sample_rate` argument directly.

## 10. G3.5 outcome (2026-11-07)

Mojo `1.3.0.dev2026110705`, `modular` `26.8.0.dev2026110705`, Apple M5.

**The emitter mode.** `MojoOptions::batch` runs `fir::batch::layout_voices`
(checked) after the flattening and emits `struct mydsp(FaustDspBatch)`:

- *Construction over one buffer.* A Mojo `Pointer` cannot be null ("Pointer
  is non-nullable"), so the struct is built over its state:
  `__init__(out dsp, voices: S32, state: Ptr[UInt8])` carves the typed arenas
  from one buffer of `state_bytes(voices)` bytes, each aligned to 8 bytes;
  `<arena>_elements()` gives each arena's elements per voice. A host
  allocates (on a device if need be), then builds. This replaces the
  per-arena `state_elements(T)` query of §3.5: one buffer is what a generic
  harness can allocate whatever the program's arena types.
- *Signatures.* `voice: S32` after `dsp` on `instance_constants`,
  `instance_reset_user_interface`, `instance_clear`, `instance_init`, `init`
  and `compute`; `class_init` unchanged, once for all voices. The canonical
  signature check runs on the signature without the voice.
- *Controls* (D2): `num_controls()`, `set_control(voice, index, value)`,
  `get_control(voice, index)` through `voice_controls` (the `fVoiceControls`
  table); `build_user_interface` describes them with `dsp.null_val` zones.
  `sample_rate` stays a shared field that every voice's `instance_constants`
  writes with the same value.
- *Code layout*: the host side of the contract is in
  `crates/codegen/src/backends/mojo/batch.rs`; the naming code (keywords,
  `snake_case`, `Renamer`) moved, unchanged, to `mojo/names.rs`, which keeps
  `mojo/mod.rs` under the 2,000-line threshold of `structure-check`.
- *No CLI switch* (D3): the lanes reach the mode through the compiler's
  `mojo_batch` example (`crates/compiler/examples/mojo_batch.rs`, clap).
- *Default output unchanged*: codegen-identity differential, baseline binary
  built at the HEAD before the change (`854e23b3`), every `.dsp` of `tests/` and
  `examples/` (634): 555 outputs byte-identical; the 79 others fail in both
  binaries for libraries absent from the search path, their messages
  differing only in the binary-relative search path they print.

Tests: `crates/codegen/src/backends/mojo/tests.rs` gains a producer-shaped
fixture (the shared fixtures declare initialised fields, which the layout
refuses) and four tests: the batch contract, the batch lifecycle conformance
(`init = class_init -> instance_init(voice)`, `instance_init(voice) =
constants -> reset -> clear`, never `class_init`), the scalar contract by
default, and a refused module surfacing as a codegen error.

**Finding: every buffer a kernel reaches must be one of its arguments.** The
first batch run gave voice 0 equal to its input. The init kernel had written
the arenas correctly (copied back, the device buffer held them), but the
compute kernel, which reaches the arenas only through the pointers stored in
the struct, read zeros and stale values (`state_int32[0]` 0 instead of
44,100). Passing the state buffer as an unused kernel argument fixed it at
once (voice 0 within 1.2e-6 of the host). On Metal, a buffer reached only
through a pointer held in another buffer is not reliably visible to a
kernel. The G1 and G2 harnesses reached the channel data the same way,
through the channel table, and worked by luck; they now pass it too
(`make mojo-gpu` rerun: 131/131).

**Correctness** (`make mojo-gpu-batch`, `archs/runimpulsemojogpubatch.mojo`):
the G1 checks on the batch contract, the host reference being a two-voice
batch instance: **131/131** (`sound` and `math` excluded, as in G1).

**Cost** (`make mojo-gpu-bench-batch`, `archs/benchmojogpubatch.mojo`, the
G2 grid; GPU columns compared with the array-of-structures sweep of the
plan §8, CPU baseline the faster of 4 and 10 threads from the same sweep):

| program | voices | per-voice layout | structure of arrays | gain | vs CPU threads |
|---|---|---|---|---|---|
| `freeverb` | 1,024 | 6.5 ms | 2.4 ms | ×2.8 | ×0.76 |
| `freeverb` | 4,096 | 103.8 ms | 3.4 ms | ×30.8 | ×2.2 |
| `karplus32` | 1,024 | 24.4 ms | 5.4 ms | ×4.5 | ×1.35 |
| `karplus32` | 4,096 | 170.8 ms | 6.5 ms | ×26.1 | ×4.8 |
| `zita_rev1` | 1,024 | 7.9 ms | 4.2 ms | **×1.9** | ×0.73 |
| `zita_rev1` | 4,096 | 111.3 ms | 5.5 ms | ×20.1 | ×2.2 |
| `osc` | 4,096 | 1.93 ms | 0.35 ms | ×5.5 | ×4.8 |

(256-frame blocks; over the four block sizes `freeverb` gains ×2.6 to ×2.9
at 1,024 voices and ×27 to ×31 at 4,096, `karplus32` ×4.4 to ×4.5 and ×25 to
×26, `zita_rev1` ×1.8 to ×1.9 and ×19 to ×20.)

**The pass criterion is met on five cells of six**: `zita_rev1` at 1,024
voices gains ×1.8 to ×1.9, under the ×2 set before G3.5. The user decided to
land the mode with this gap recorded. Other limits measured: at 64 and 256
voices the structure of arrays is mostly slower than the per-voice layout
for the large-state programs (×0.6 to ×0.9; `freeverb` at 64 voices of 64
frames is the exception, ×1.2), and `APF`, with 32 bytes of state, gains
nothing (×0.8 to ×1.2). `osc`'s table is now shared: 32 bytes per voice
instead of 262,180, which also lets it run 16,384 and 65,536 voices (3.2 ms
for 65,536 voices of 256 frames, within the 5.8 ms budget).

**Where the device now beats the CPU threads** (from 4,096 voices unless
said): `freeverb` ×1.9 to ×4.6, `zita_rev1` ×2.1 to ×3.4, `karplus32` from
1,024 voices (×1.3 to ×1.4, then ×4.4 to ×5.1), `osc` from 1,024 voices at
256 frames and above (up to ×5.6), `APF` ×3.5 as in G2. None of them did with
the per-voice layout, apart from `APF`.

