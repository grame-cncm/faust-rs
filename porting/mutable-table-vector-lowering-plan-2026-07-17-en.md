# Mutable Table Vector Lowering Plan

Date: 2026-07-17
Status: complete; E1.1 in `d1292292`, E1.2 qualified on 2026-07-17
Scope: `rwtable` admission, lowering, and verification in the checked vector pipeline

## 1. Objective and baseline

After E4 the corpus stands at 88/93 in all 16 modes. `table1` and `table2` are
the `FRS-VEC-FALLBACK-PURE` class:

```
signal 55 is effectful and cannot enter pure P5.2 lowering: SIGWRTBL(...);
effects=[..., WriteTable(55)]
```

The admission gate (`lower.rs`, `effects_supported`) accepts `ReadTable` and
`WriteTable` atoms only through `readonly_table_signal`, i.e. only for a
`WrTbl` with nil write ports. A live-port `rwtable` is rejected wholesale, and
`lower_readonly_table_definition` independently rejects it. E0 removed the
phantom write that `rdtable` used to contribute, so what remains is exactly the
genuine mutable class.

The scalar backend defines the target shape, measured on both cases:

- read-only tables stay `const static` arrays (`fTbl49`, `fTbl54`);
- the mutable table is a DSP-struct array field (`double fTbl55[8];`);
- its initial content is written element-wise in `instanceConstants`
  (`fTbl55[0] = 0.0; ...`). This holds even for `table2`'s recursive
  generators: the SIGGEN interpreter const-folds the generator at compile time,
  so `iTbl79[0] = 1; iTbl79[1] = 2; ...` - no runtime init loop exists in
  either case;
- `compute` performs one `StoreTable` per sample with a wrapped write index,
  ordered before the same-sample reads of that table (`rwtable` semantics:
  `fTbl55[((iTemp0 % 8) + 8) % 8] = fTbl54[iWave54];` precedes
  `output1[i0] = fTbl55[iTemp0];`).

Unlike delay rings, a table is addressed absolutely, so the struct field
crosses chunk boundaries with no copy-in/copy-out and no permutation state.

## 2. Design

### Admission

`effects_supported` gains a mutable-table arm: `ReadTable(t)`/`WriteTable(t)`
are admissible when `t` names a `WrTbl` whose size is a constant and whose
generator the SIGGEN interpreter can evaluate - the same evaluator the scalar
path and `ensure_readonly_table` already use. The read-only predicate
`wrtbl_is_readonly` is untouched; mutability is a new arm, not a weakening.

### Lowering

On the model of `lower_bargraph` (the existing admitted effectful signal):

- the `WrTbl` signal declares a struct array field with a canonical name
  derived from its signal id, registered with the module's state declarations
  so the DSP-struct check covers it;
- its initial content is evaluated by the SIGGEN interpreter and emitted as
  element-wise stores appended to the control statements that form
  `instanceConstants`, matching the scalar lifecycle placement;
- in the writer's loop body the node lowers its write index and write value
  and emits one `StoreTable`; the node itself has no runtime value, as in the
  read-only path;
- `RdTbl` over a mutable table lowers to `LoadTable` on the struct field,
  reusing the existing read machinery with a mutable-table source instead of
  the `readonly_tables` registry.

### Ordering

`effects_conflict` already declares `ReadTable(a)`/`WriteTable(a)` conflicting
on one table. Writer and readers are direct performers - `WrTbl` performs its
`WriteTable`, each `RdTbl` its `ReadTable` - so E4's attribution emits their
events and the event certificate orders them exactly as it orders bargraph
writes: co-located in one serial loop or fission-safe across loops, fail-closed
otherwise. Same-sample write-before-read inside one loop is carried by the data
edge from each `RdTbl` to its `WrTbl` operand. The E4 per-kind carrier
argument extends to mutable tables unchanged: a carrier of a table atom
performs no table operation, and its ordering after the performer is the same
data/execution-condition path as every other kind.

## 3. Independence obligations

E0's `verify_readonly_table_stores` currently reads "no `StoreTable` into any
declared table", sound only while every table in a checked vector module is
read-only. E1 replaces it with a classification by declaration site, derived
from the emitted FIR alone:

- E1.C1: a table declared in `static_declarations` with initializers is
  read-only: zero `StoreTable` to its name anywhere in `compute`.
- E1.C2: a table declared as a DSP-struct field is mutable: the number of
  physical `StoreTable` operations on its name in `compute` must equal the
  number of claimed direct `WriteTable` performers for it in the plan, in both
  directions - the table analogue of E4's UI-write attribution check, and the
  extension E4 recorded as mandatory before admitting this class.
- E1.C3: the element-wise initialization of each mutable table must appear in
  `instanceConstants` and cover every index exactly once, with values equal to
  the SIGGEN evaluation the certificate claims.

These read `DeclareTable`/struct fields/`StoreTable` from the assembled FIR and
consult nothing the effect model or the lowerer produced.

## 4. Rejecting mutations and focused tests

- M1: a forged store into a statically declared read-only table (E0's mutation,
  retained).
- M2: a mutable table with a claimed `WriteTable` performer and no emitted
  `StoreTable`.
- M3: two claimed performers for one emitted store.
- M4: an `instanceConstants` init dropping one element of a mutable table.
- M5: writer and reader of one table forced into loops whose vector order
  reverses their scalar dependence must stay rejected
  (`FissionSafeViolation`), preserving the two-performer conflict.
- P1: a corpus-independent fixture with the `table1` shape - one `rdtable`
  and one `rwtable` sharing a waveform and a recursive index - certified and
  scalar/vector bit-exact under both loop variants, all four strategies, and a
  chunk size that does not divide the block. Admission rejects this shape
  outright today, so certification alone discriminates - the vacuity that
  disqualified E4's P2 does not arise.

## 5. Rollout

### E1.0 - plan

This document. No compiler behavior change.

### E1.1 - admission, lowering, and checks

The admission arm, the mutable lowering, E1.C1-C3 replacing the E0 check, and
M1-M5/P1. The widening and its independent checks land together.

### E1.2 - corpus qualification

Full 16-mode sweep and baseline refresh (expect 88 -> 90), the 8-case native
C++ oracle matrix for each of `table1` and `table2` with `.ir` deleted first
and `filesCompare` counts asserted, byte-identity for every previously
certified DSP against the pre-change compiler, compile-budget, and the
workspace gates.

## 6. Risks and mitigations

- Same-sample write/read order is semantic, not stylistic: `rwtable` reads
  must observe the current sample's write. Bit-exactness against scalar and
  the 60,000-frame oracle matrix are the arbiters; the data edge from read to
  writer is the structural carrier.
- The SIGGEN evaluation must agree between the certificate and
  `instanceConstants` emission; E1.C3 pins it element-wise.
- `table2` packs four `rwtable`s with distinct sizes and int/real content in
  one program; the fixture matrix must include an int-content table so
  `iTbl`/`fTbl` typing is exercised.
- Anything the SIGGEN interpreter cannot evaluate (foreign calls -
  `subcontainer1`) stays fail-closed with its existing diagnostic; E1 must not
  touch that boundary, which belongs to E3.
