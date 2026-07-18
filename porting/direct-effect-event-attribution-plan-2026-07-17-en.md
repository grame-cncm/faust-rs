# Direct Effect Event Attribution Plan

Date: 2026-07-17
Status: complete; E4.1-E4.2 in `241b1e98`, E4.3 qualified on 2026-07-17
Scope: which signal an effect event is attributed to in the vector event model

## 1. Objective and baseline

After E0 the corpus stands at 87/93 certified in all 16 modes, with 5 fallbacks
and 1 error. `mixer` is the last `FRS-VEC-FALLBACK-EVENTS` case:

```
vector execution reverses scalar dependence 155 -> 747
```

Both events are `WriteUi(2)`, one in `Loop(0)` at sample 0 (signal 1661) and one
in `Loop(18)` at sample 1 (signal 1657). The model reports a write-after-write
on one UI zone, and vector loop fission runs `Loop(18)` before `Loop(0)` and
reverses it.

Control 2 has exactly one writer. Probing the effect analysis reports **one**
direct writer of `WriteUi(2)` (signal 1514) and **38** transitive carriers of
it. Neither 1661 nor 1657 is 1514: both merely contain the vumeter in their
subtree. The emitted code agrees - `mixer` declares 10 distinct bargraph zones
and stores each exactly once in `compute`, so `fVbargraph2` has one writer and
nothing to reorder.

The event table is built from the accumulated effect set
(`events.rs`, `independently_expected_event_keys`):

```rust
for (effect_index, effect) in signals[&definition.signal_id].effects.iter().enumerate() {
```

`plan.signals[].effects` is the conservative transitive projection, documented
in `SignalUses` as "Sorted conservative compute-time effects, including
non-`Gen` children". Every materialized carrier of an effect therefore emits an
event for it, and any two carriers landing in different loops manufacture a
conflict on an operation that happens once. `mixer` has 48 loops and 38
carriers of one zone, so it finds one.

Like E0 this is a fail-closed imprecision: the model over-reports operations, so
it rejects programs it cannot order rather than mis-ordering them. It costs
coverage, not correctness.

## 2. The projection already exists

`SignalUses` carries both sets, and the direct one exists for exactly this
purpose:

```rust
/// Sorted conservative compute-time effects, including non-`Gen` children.
pub effects: Vec<EffectAtom>,
/// Sorted effects performed by this node itself, excluding child effects.
///
/// This internal projection lets scalar scheduling orient actual effect
/// operations without paying a quadratic cost over every transitive
/// effect carrier in the signal graph.
direct_effects: Vec<EffectAtom>,
```

Scalar scheduling already orients on actual operations. `direct_effects` is
private and reaches neither `DecorationRecord` nor the vector plan, so the
vector event model can only see carriers. This phase carries the projection
through to the event model; it introduces no new analysis.

The accumulated set must stay. `duplicable: effects_duplicable(&record.effects)`
legitimately needs it: a signal whose *subtree* has effects must not be
duplicated, and that is what keeps performers unique. Only event emission moves
to the direct projection.

## 3. Correctness argument

Removing carrier events removes ordering constraints, so the real obligations
must be shown to survive. Three claims, per effect atom `E`:

- C-a: two direct performers of conflicting effects both keep their events and
  stay ordered. Nothing changes for them.
- C-b: a carrier of `E` does not perform `E`, so no order between two carriers
  of `E` is required. This is what `mixer` currently manufactures.
- C-c: a carrier of `E` still executes after the performer it inherits `E`
  from. The accumulated set is the least union fixed point propagated along
  `condition_children` of the dependency projection, so a signal holding `E`
  accumulated but not directly has a data or execution-condition path to some
  signal that performs `E`. Both edge kinds already impose order, independently
  of any effect atom.

C-c is the load-bearing claim and must be checked, not assumed: it is the exact
analogue of E0's argument that fill-before-read survived as a data edge because
`waveform5` planned with `effect_edges=0`. Here the claim is per effect kind and
the plan must not take it on faith for `ReadState`/`WriteState`,
`ReadTable`/`WriteTable`, `WriteUi`, `WriteOutput`, or `Foreign` at once.
`Foreign` deserves particular care: an unknown-purity foreign call is a
performer, and E3 will revisit its identity.

## 4. The independence problem

E0 hit this and E4 hits it harder. A producer that records `direct_effects` and
a checker that reconstructs them both call the same `direct_effects` function,
so their agreement proves nothing about the projection itself. The independent
evidence must again come from the emitted FIR:

- E4.C1: for every effect resource and every loop, the certificate claims a
  direct performer in that loop if and only if the assembled FIR contains a
  physical operation on that resource in that loop. UI zones are `StoreVar` on
  the zone field, tables are `StoreTable`/`LoadTable`, state is the checked
  state plan's declarations, outputs are the output stores.
- E4.C2: the count of physical operations on one resource per sample matches
  the count of claimed direct performers. `mixer` fails today's model precisely
  because 38 claimed operations correspond to 1 physical store.

E4.C1 is what would catch a misprojection, and it must land with the attribution
change rather than after it. E4.C2 subsumes the `mixer` case as a corpus-visible
instance.

## 5. Versioned data model

`DecorationRecord` gains a `direct_effects: Vec<EffectAtom>` field, canonically
sorted and a subset of `effects`. The vector plan `signalRecord` gains the same
field, so the certificate moves v3 -> v4 with
`porting/schemas/vector-verification-certificate-v4.schema.json` requiring
`direct_effects` alongside the existing `effects`. Both checkers must reject a
certificate whose `direct_effects` is not a sorted subset of its `effects`; that
subset relation is cheap, total, and independent of how either set was derived.

No CLI, ABI, lifecycle, or contraction-policy change.

## 6. Rejecting mutations and focused tests

- M1: a certificate whose `direct_effects` is not a subset of `effects`.
- M2: a certificate whose `direct_effects` is unsorted or contains duplicates.
- M3: a forged certificate promoting a carrier to a direct performer; E4.C1
  must reject it, because no physical operation exists in that loop.
- M4: a forged certificate demoting the real performer to a carrier; E4.C1 must
  reject it, because a physical operation exists with no claimed performer.
- M5: two genuine performers of one resource must keep their mutual order; a
  mutation dropping one of their events must be rejected.
- P1: a fixture with one effect performer and several carriers of it spread
  across loops, asserting one event and no manufactured conflict.
- P2: a corpus-independent compiler fixture reproducing the `mixer` shape - one
  `attach`ed bargraph whose value feeds two consumers that land in different
  loops - under both loop variants and all four scheduling strategies.

## 7. Rollout

### E4.0 - plan

This document. No compiler behavior change.

### E4.1 - projection and schema

`direct_effects` exposed, carried through `DecorationRecord` and the plan
signalRecord, schema v4, subset/canonicity checks, M1/M2/P1. Event emission
still uses the accumulated set: this step changes no certification outcome and
is separately committable.

### E4.2 - attribution and FIR obligation

Event emission moves to the direct projection, with E4.C1/E4.C2 over the
assembled FIR, plus M3/M4/M5/P2. The attribution change and its independent
check land together; the attribution alone is not a shippable state.

### E4.3 - corpus qualification

Full 16-mode sweep, oracle matrix for `mixer`, and the workspace gates. Not
claimed by E4.1 or E4.2.

## 8. Acceptance gates

- `mixer` certified in all 16 modes; no DSP loses certification; expected
  87 -> 88 with `FRS-VEC-FALLBACK-EVENTS` eliminated, leaving three classes:
  mutable tables (E1), soundfile (E2), and foreign functions (E3).
- Generated code byte-identical for every already-certified DSP, compared
  against a build of the pre-change commit: this phase changes which programs
  are admitted, never what they compile to.
- Scalar/vector interpreter bit-exactness for the `mixer` shape over both loop
  variants, all four scheduling strategies, and a chunk size that does not
  divide the block.
- The 60,000-frame native C++ oracle matrix for `mixer` at `-lv 0/1 x -ss 0..3`,
  run with its `.ir` outputs deleted first and its `filesCompare` invocation
  count asserted. The `make` harness reports success from cached outputs
  otherwise.
- Event counts must fall on the UI-heavy corpus. `mixer` builds 1,702 events
  today with 38 carriers of one zone; if the count does not drop materially,
  the projection is not being applied where it was diagnosed.
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace --all-targets`, `vector-coverage-check`, and
  `vector-compile-budget-check`.

## 9. Risks and mitigations

- Removing events is a widening change: a wrong projection admits an unsound
  program rather than rejecting a sound one. E4.C1/E4.C2 are the mitigation and
  gate E4.2, not E4.3.
- C-c may not hold uniformly across effect kinds. It must be established per
  kind before the attribution flips, and any kind whose carriers lack a
  guaranteed ordering path keeps its carrier events and stays fail-closed with
  its diagnostic.
- The accumulated set has other consumers than events. `duplicable` is the
  known one and must keep reading `effects`; sweeping every consumer is part of
  E4.1 rather than an assumption.
- Event-count reduction may shift compile cost. The budget basket has no
  UI-heavy entry, and `mixer` is not in it, so a regression there would be
  invisible; measure `mixer` compile time explicitly in E4.3 rather than
  trusting the basket.

## 10. Relation to the other phases

- E1 (`table1`/`table2`, mutable `rwtable`) is independent and may land first.
  It must extend `verify_readonly_table_stores`, which E0 left reading "no
  store into any declared table".
- E2 (`sound`) is independent.
- E3 (`math`, `subcontainer1`) touches `Foreign` effect identity and should be
  sequenced after E4, whose C-c argument must already have decided how a
  foreign performer is distinguished from its carriers.
