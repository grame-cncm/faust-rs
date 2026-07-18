# Soundfile Vector Lowering Plan

Date: 2026-07-18
Status: complete; E2.1 in `c04fc58a`, E2.2 qualified on 2026-07-18
Scope: soundfile data reads in the checked vector pipeline

## 1. Objective and baseline

After E1 the corpus stands at 90/93 in all 16 modes. `sound.dsp` is the
`FRS-VEC-FALLBACK-UI` class, rejected before any vector stage runs by an
explicit guard in `build_verified_vector_module_with_evidence`:

```
soundfile control 0 has checked UI lifecycle state, but vector sound data
lowering is not yet certified
```

## 2. Diagnosis

Everything around the data reads already exists; only the reads themselves
are missing from P5.2:

- The scalar backend defines the target shape: a `Soundfile*` struct field
  registered by `addSoundfile` in `buildUserInterface`, a block-invariant
  `fLength[part]` read, and per-sample per-channel
  `fBuffers[chan][fOffset[part] + idx]` reads. Compute never writes soundfile
  state: the data is immutable once loaded.
- The effect model already agrees: `SigMatch::Soundfile` contributes no
  atoms, and `SoundfileLength`/`SoundfileRate`/`SoundfileBuffer` are pure
  dependency projections. Unlike E0 no correction is needed - the model is
  right, matching the read-only-table precedent whose ordering argument
  carries over: content loading is lifecycle-time, operand order rides data
  edges, and there is nothing to conflict with.
- The FIR vocabulary exists (`AddSoundfile`, `LoadSoundfileLength/Rate/
  Buffer`), the scalar lowering in `module/ui_lowering.rs` is the exact
  template, and the FIR checker already ties soundfile access to `Sound`
  struct fields (`FIR-SF01`).
- The vector UI program already declares the `Sound` struct field and emits
  `addSoundfile` - the guard's own message concedes the lifecycle is checked,
  and `verify_final_module` pins `buildUserInterface` by exact block equality.
- The native oracle is meaningful: the scalar C++ harness passes `sound.dsp`
  against `reference/sound.ir` today, so the vector matrix inherits a real
  numeric arbiter.

## 3. Design

- Remove the early soundfile guard.
- Four P5.2 lowering arms mirroring the scalar template:
  - `Soundfile(control)` resolves its vector UI zone with the same
    kind-checked path as `lower_ui_input` and loads the `Sound` struct field;
  - `SoundfileLength(sf, part)` and `SoundfileRate(sf, part)` lower their
    part and emit `load_soundfile_length`/`load_soundfile_rate` on the zone
    name;
  - `SoundfileBuffer(sf, chan, part, idx)` lowers its operands and emits
    `load_soundfile_buffer` with the node's FIR type.
- No effect-model, certificate, plan, event, or state change: the signals are
  pure reads and rate placement (block-invariant lengths in the control
  region, buffer reads in sample loops) falls out of the existing decoration
  and planning machinery.

## 4. Independence obligation

The admission widens what compute may contain, so the emitted FIR must carry
the read-only claim independently:

- E2.C1: no `StoreVar` to any `Sound`-typed DSP-struct field anywhere in the
  assembled `compute` body. The field set is derived from the emitted struct
  declarations alone, exactly as the mutable-table and readonly-table checks
  derive theirs, and a rejecting mutation forges such a store.

Lifecycle coverage needs no new obligation: the `Sound` field and its
`addSoundfile` registration are already pinned by the existing exact-match
checks.

## 5. Rejecting mutations and focused tests

- M1: a forged `StoreVar` into a `Sound` struct field is rejected by E2.C1,
  naming the field.
- P1: a corpus-independent compiler fixture with the `sound.dsp` shape
  (recursive index into a multi-part `soundfile`, dropped length/rate
  outputs) certified under both loop variants and all four strategies. The
  guard rejects the shape outright today, so certification alone
  discriminates. Interpreter bit-exactness is asserted only if the
  interpreter lane runs soundfiles at all; the numeric arbiter is the native
  matrix.

## 6. Rollout

### E2.0 - plan

This document. No compiler behavior change.

### E2.1 - admission, lowering, and check

Guard removal, the four arms, E2.C1 with M1, and P1. Lands together.

### E2.2 - corpus qualification

Full 16-mode sweep and baseline refresh (expect 90 -> 91), the 8-case native
C++ oracle matrix for `sound.dsp` with `.ir` deleted first and the
`filesCompare` count asserted, byte-identity for every previously certified
DSP against the pre-change compiler, compile-budget, and the workspace gates.

## 7. Risks and mitigations

- The impulse architecture supplies deterministic dummy soundfile content;
  the oracle validates the read plumbing, not file I/O, which is exactly the
  compute-side contract this phase touches.
- `sound.dsp` drops its length outputs (`!,!`), so the fixture keeps a live
  length read to cover `LoadSoundfileLength` in the certified path.
- If the interpreter lane lacks soundfile support, the 16-mode sweep still
  certifies structurally; numeric coverage stays with the C++ matrix, stated
  honestly in the journal.
