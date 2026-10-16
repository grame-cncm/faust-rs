# Late state join in normalized sums: port of upstream `d4d51e5ed`

Date: 2026-10-16. Status: implemented (see the journal entry of the same day).

## Finding

faust-coopiano's sympathetic register never went back to sleep after the first
pedal (faust-coopiano `faust-rs-issues/RAPPORT.md`, point 15). Its rate-limited
damper openness `o + clamp(-st, st, 1 - c - o)` settled at 1.1e-17 instead of 0.
faust-rs normalizes `1 - c - o` as `(1 - o) - c`, so `1 - o` rounds and the
recursion stops at that rounding error; Faust 2.90.10 normalizes it as
`(1 - c) - o`, hoists `1 - c` to block rate and reaches 0 exactly.

faust-rs is **not** diverging from its parity target here. The pinned C++
reference (`master-dev-ocpp-od-fir-2-FIR19`, `8eebea429`, Faust 2.84.3) prints
the same normal form as faust-rs (`-norm`: `-(-(1, o), c)`). The upstream change
is Yann Orlarey's commit `d4d51e5ed` of 2026-08-09, "order: the audio-recursive
class (level 4) — late state join in the sums", posterior to the reference.

Upstream's own evaluation of that commit, on its corpus: 156 programs touched,
136 bit-exact, 20 with rounding differences (at most 2e-05), no gross deviation,
geometric-mean speed 0.995, with gains on the filters whose recurrence crossed a
foreign addition (oberheimBPF 0.74, oberheimLPF 0.79, sallenKey2ndOrderHPF
0.88; worst loser 1.03).

## Decision

The user chose on 2026-10-16 to port `d4d51e5ed`, as `gApplyMemo` was ported
before it although posterior to the reference. faust-rs then follows upstream
2.90, not the pinned reference, on this point: registry entry DIFF-BEH-023.

## What upstream changed

- `sigs::sigOrder(t)` returns a fifth level, 4, when a recursive node (SYMREC,
  DEBRUIJN or DEBRUIJNREF) occurs in `t`: tlib's `kContainsRec` bit, "some state
  reaches me".
- `aterm::normalizedTree`: the order-4 terms are kept out of the descending fold
  of orders 3 to 1 and join last, as the right operand (negative terms, then
  positive ones). A two-term sum `{slow, state}` keeps its historical operand
  order `slow + state`; only sums with several terms off the state path change
  shape, and their slow part becomes one hoistable subtree.
- `mterm::normalizedTree`: the level is capped to 3 when products are grouped,
  so their shape does not change.
- Not stated in the commit message but part of its behaviour:
  `mterm::complexity` weighs a factor by `1 + sigOrder`, uncapped, so a factor
  that depends on state weighs one more than an audio-rate one. That steers
  `greatest_divisor` and the factorization loop of `normalizeAddTerm`.
- The other readers of `sigOrder` (`normalize.cpp`'s `sigOrder(x) < 2` tests, a
  diagnostic in `compile_scal.cpp`) see no change: their threshold sits below
  the new level.

## Port

- `tlib::TreeArena` computes the same bit when a node is interned. A node
  contains a recursion when its tag is `SYMREC`, `SYMREF`, `DEBRUIJNREC` or
  `DEBRUIJNREF`, or when one of its children contains one. The arena exposes it
  as `contains_recursion(id)`, read in O(1) with a shared borrow, which is what
  `complexity` needs. `SYMREF` is included because faust-rs writes a
  self-reference as `proj(i, SYMREF(var))`, where C++ uses the rec node itself.
  The bit must stay true inside a definition as outside it.
- `normalize`: one helper, `state_order(arena, types, t)`, returns 3 when `t`
  contains a recursion and `sig_order` (0 `Konst`, 1 `Block`, 2 `Samp`)
  otherwise.
  - `Aterm::normalized_tree` buckets on it and joins bucket 3 last, as the
    right operand.
  - `Mterm::complexity` weighs factors with it.
  - The grouping of products keeps `sig_order`, which is the cap.

## Deliverables and pass criteria

1. Unit tests:
   - `tlib`: the bit on each recursion tag, its propagation, and its absence on
     plain trees.
   - `normalize`:
     - `1 - c - o` gives `(1 - c) - o`;
     - `{slow, state}` keeps `slow + state`;
     - a sum mixing state, audio and slow terms;
     - the complexity weight.
2. End-to-end: the faust-coopiano reproducer `r16` hoists `1 - c` to block rate
   in the generated C++, and its recursion reaches 0.
3. Corpus differential, base against new, over every `.dsp` under `tests/` and
   `examples/` (scalar, `-vec`, `-double`). The changes must be confined to sums
   that contain a state term, and exit codes and stderr must not change.
4. `impulse-tests` on every backend: no new failure (rounding stays within the
   harness tolerance).
5. Compilation cost: a manual before/after measure of both baskets, since the
   gate cannot calibrate on this machine. The intern-time bit must not show.
6. All of AGENTS.md §3, the code-graphs baseline (one new public method in
   `tlib`), registry DIFF-BEH-023, and a journal entry.

## Risks

- **Tag ids.** Interning the four recursion tags when the arena is created
  shifts every later tag id by four. Nothing may depend on tag-id values;
  criterion 3 checks it.
- **Intern cost.** The flag costs one byte per node and one OR over the
  children of each new node. Criterion 5 checks it.
- **Golden C++ snapshots.** Snapshots taken from the pinned reference now
  differ where sums change. The registry entry records that this is
  deliberate.
