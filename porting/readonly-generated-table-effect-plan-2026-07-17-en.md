# Read-Only Generated Table Effect Plan

Date: 2026-07-17
Status: complete; E0.1-E0.3 implemented and qualified on 2026-07-17
Scope: compute-time effect identity of `rdtable` generated tables

## 1. Objective and baseline

After Phase D3 qualification the corpus stands at 86/93 certified in all 16
modes, with 6 fallbacks and 1 error:

- `FRS-VEC-FALLBACK-EVENTS`: `mixer`, `waveform5`;
- `FRS-VEC-FALLBACK-PURE`: `math`, `table1`, `table2`;
- `FRS-VEC-FALLBACK-UI`: `sound`;
- transform error `FRS-SFIR-0004`: `subcontainer1`.

`waveform5` was scheduled as part of Phase E4 ("residual dependence
reversal"), on the assumption that its event certificate needed widening. Live
diagnosis disproved that assumption: the reversal is reported against a table
write that the compiler never emits. This phase corrects the effect identity
instead, and is a prerequisite for E1 rather than a member of E4.

The signal-level effect model attaches a write to every `SIGWRTBL` node
(`analysis.rs`, `direct_effects`):

```rust
SigMatch::WrTbl(_, _, _, _) => BTreeSet::from([EffectAtom::WriteTable(sig.as_u32())]),
```

`rdtable` and `rwtable` both lower to `SIGWRTBL`. They differ only in their
write arguments: `rdtable` carries a nil write index and nil write value,
`rwtable` carries real ones. The arm above ignores that distinction, so every
`rdtable` is modelled as a compute-time writer of its own table.

## 2. Evidence

Four independent observations, all reproduced on `2d0a2a49`:

1. Probing `direct_effects` over the corpus separates the two shapes exactly.
   `waveform5` signal 12: `write_index_nil=true write_value_nil=true`.
   `table1` signal 49 (`rdtable`): `true/true`. `table1` signal 55
   (`rwtable`): `false/false`.
2. The emitted code contains no runtime table store for either shape's
   read-only tables. `waveform5` scalar emits
   `const static int iTbl12[5] = {10, 20, 30, 40, 50};`, and certified
   `waveform6` emits `const static int iVecTbl13[6]` in scalar and vector
   alike. A store count over both outputs is zero.
3. The pure vector lowerer already draws the distinction this model misses.
   `lower_readonly_generator` (`lower.rs`) admits a generator only when
   `is_nil(write_index) && is_nil(write_value)`, and emits a read-only table
   for it. The effect model and the lowerer therefore contradict each other on
   the same signal, and the lowerer is the one that matches the emitted FIR.
4. Removing the write from the nil-write arm certifies `waveform5`
   (86 -> 87, `FRS-VEC-FALLBACK-EVENTS` 2 -> 1) with
   `cargo test -p transform --lib` still at 375/375. The existing
   `direct_effects` table assertion keeps passing because its fixture uses a
   real write.

The rejected dependence itself is the anti-dependence
`ReadTable(12) @ Loop(0) sample 0 -> WriteTable(12) @ Loop(1) sample 1`.
`waveform5` differs from certified `waveform6` only in that its table index is
a UI slider (`4*hslider(...)`, block-invariant, hoisted to `iSlow0`) rather
than an input signal (`4*abs(x)`). That invariance splits the program into two
loops (`loops=2 data_edges=1 effect_edges=0`) where `waveform6` keeps one
(`loops=1`), and only a two-loop program can have its loops fissioned into a
reversing order. `waveform6` is not safer; it is merely unfissionable.

## 3. Correctness argument

Dropping the write atom must not lose a real ordering obligation.

The generator must still run before any read of its table. That obligation is
carried by the data edge from the `RdTbl` node to its `WrTbl` operand, not by
the effect: `waveform5` plans with `effect_edges=0`, so the fill-before-read
order survives with no effect atom at all. This is structural rather than
incidental - a `RdTbl` always names its table signal as an operand.

The nil-write predicate is decidable on the prepared signal graph and is
already the lowerer's admission rule, so the two components converge on one
definition instead of disagreeing.

Table content generation is init-time. A read-only generated table is filled
before `compute` (constant-folded to a `const static` array, or filled in
class initialization for a non-constant generator). The effect model describes
compute-time execution, where the table is immutable.

## 4. The independence problem

Phases C and D relied on a producer and an independent checker reconstructing
the same facts from disjoint code. `direct_effects` is shared by both: the
producer builds events from it and the checker reconstructs events from it.
Correcting it therefore moves producer and checker together, and a wrong
correction would be invisible to their agreement. Today both wrongly believe
`rdtable` writes, so they jointly reject and stay fail-closed - which is why
this bug costs coverage rather than correctness.

The plan must not add a second opinion drawn from the same model. The
independent evidence has to come from the assembled FIR:

- E0.C1: for every table the certificate treats as read-only, the assembled
  vector FIR must contain no `StoreTable` to that table id anywhere in the
  compute body, across every region and both loop variants. This checks the
  claim against emitted code rather than against the model that produced it.
- E0.C2: a table with any non-nil write argument must keep its
  `WriteTable` atom, and the read-only classification must be rejected for it.

E0.C1 is the load-bearing obligation: it is the only check that would catch a
misclassification, since the effect model can no longer contradict itself.

## 5. Producer construction

Split the `SIGWRTBL` arm of `direct_effects` on the nil-write predicate:

- nil write index and nil write value: no `WriteTable` atom. The node keeps
  whatever effects its generator subtree contributes; only the table-write
  atom of the `WrTbl` node itself is dropped.
- otherwise: unchanged `WriteTable(sig)`.

Factor the predicate into one shared total function over the arena so that the
lowerer's admission rule and the effect model cannot drift apart again, in the
spirit of F3's shared checker vocabulary. The lowerer keeps its own admission
check; both call the same predicate.

No certificate schema change is required: this phase removes atoms from an
existing set rather than adding a field. The v3 vector plan certificate and the
event certificate are unaffected in shape.

## 6. Rejecting mutations and focused tests

- M1: forge a `WrTbl` with a non-nil write index and nil write value; the
  predicate must classify it as a writer, not read-only.
- M2: same with nil index and non-nil value.
- M3: forge an assembled FIR that stores to a table classified read-only;
  E0.C1 must reject it.
- M4: keep the existing `direct_effects` rwtable fixture green, proving the
  writer path is untouched.
- P1: a positive fixture pairing one `rdtable` and one `rwtable` over distinct
  tables in one program, asserting the read-only one carries no write atom and
  the mutable one does.
- P2: a corpus-independent compiler fixture reproducing the `waveform5` shape
  (block-invariant table index forcing two loops) under both loop variants and
  all four scheduling strategies.

## 7. Rollout

### E0.0 - plan

This document. No compiler behavior change.

### E0.1 - shared predicate and effect correction

The shared nil-write predicate, the split `direct_effects` arm, the lowerer
rewired onto the predicate, and M1/M2/M4/P1.

### E0.2 - assembled-FIR obligation

E0.C1 over the assembled vector FIR, plus M3 and P2.

### E0.3 - corpus qualification

Full 16-mode sweep, oracle matrix for every newly certified DSP, and the
workspace gates. Not claimed by E0.1 or E0.2.

## 8. Acceptance gates

- `waveform5` certified in all 16 modes; no DSP loses certification;
  expected 86 -> 87 with `FRS-VEC-FALLBACK-EVENTS` reduced to `mixer` alone.
- Scalar/vector interpreter bit-exactness for `waveform5` and `waveform6` over
  both loop variants, all four scheduling strategies, and a chunk size that
  does not divide the block.
- The 60,000-frame native C++ oracle matrix for `waveform5` at
  `-lv 0/1 x -ss 0..3`, run with its `.ir` outputs deleted first and its
  `filesCompare` invocation count asserted. The `make` harness reports success
  from cached outputs otherwise; an earlier D3 run exited 0 having executed
  zero comparisons.
- Generated code byte-identical for every already-certified DSP: this phase
  must change which programs are admitted, never what they compile to.
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace --all-targets`, `vector-coverage-check`, and
  `vector-compile-budget-check`.

## 9. Consequences for Phase E

- E4 splits. `waveform5` leaves it entirely; the residual
  `FRS-VEC-FALLBACK-EVENTS` case is `mixer` alone, whose reversal involves UI
  writes from `component()` vumeters crossing voices and needs its own
  diagnosis before any certificate work.
- E1 shrinks and must follow E0. `table1` carries both shapes: signal 49
  (`rdtable`) contributes a phantom write and signal 55 (`rwtable`) a real
  one. Designing the write-effect resource before E0 would size it against a
  conflict surface that partly does not exist.
- E2 (`sound`) and E3 (`math`, `subcontainer1`) are untouched.

## 10. Risks and mitigations

- A read-only generated table whose generator is itself effectful keeps those
  generator effects; only the `WrTbl` node's table-write atom is dropped.
  P1 covers the pairing, and E0.C1 covers the emitted result.
- Dropping an atom widens what the checkers admit, so a misclassification
  would admit an unsound program instead of rejecting a sound one. E0.C1 is
  the mitigation and must land with, not after, the effect change; E0.1 alone
  is not a shippable state.
- If any DSP outside the corpus relies on `rdtable` being ordered by effect
  rather than by data, `effect_edges` would drop an edge it needs. No corpus
  case does, and the data edge is structural, but the byte-identical gate over
  all 86 certified DSPs is what would surface it.
