# G3: per-voice state as a structure of arrays. Design

Date: 2026-11-07

Status: **design; decisions D1 to D4 taken 2026-11-07 (§6); G3.0 done,
G3.2 next; nothing implemented.**
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
   field, a remaining sub-module, a `-vec` module (scope: scalar first).

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
