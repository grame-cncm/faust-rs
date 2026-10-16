# Normalize — Isomorphic Recursive Group Merging Plan

**Date:** 2026-05-01
**Status:** Implemented 2026-05-01 (`crates/normalize/src/rec_merge.rs`); opening cost corrected 2026-10-16 (see Complexity)
**Scope:** Add a pre-simplification pass in `crates/normalize/` that detects
structurally isomorphic `SYMREC` groups and unifies them to a single canonical
representative, enabling downstream algebraic simplification (e.g. `x - x → 0`)
across signal trees produced by independent constructions.

**Motivating case:** `fad_rec.dsp`

```faust
process_fad = step ~ (_, _);           -- manual FAD: builds SYMREC W0
fad(process_original, (g))             -- auto FAD:   builds SYMREC W2

process = _<: (process_fad : *(-1),*(-1)), fad(process_original, (g)) :> _,_;
-- Expected: both outputs are zero.  Actual: two live recursion arrays.
```

Both groups implement the same first-order IIR and its gradient. Because they
are constructed by separate pipeline stages they receive different symbolic
variable names from `de_bruijn_to_sym` (`W0`, `W2`, …) and therefore different
`SigId`s. The simplifier's `x - x → 0` rule (commit `215b688`) cannot fire:
`Proj(0, W0-group) ≠ Proj(0, W2-group)` as `SigId`s.

---

## Representation: SYMREC / SYMREF (after de_bruijn_to_sym)

Propagation (`crates/propagate`) builds recursive signals in de Bruijn form:

```
DEBRUIJNREC(body)  — binder
DEBRUIJNREF(level) — reference to enclosing binder at given De Bruijn level
```

`signal_prepare.rs` calls `tlib::de_bruijn_to_sym` on the entire cloned
signal forest before any other pass.  The converter assigns a **fresh symbolic
variable** (`W0`, `W1`, …) to each `DEBRUIJNREC` it encounters and rewrites
every corresponding `DEBRUIJNREF` to `SYMREF(var)`:

```
SYMREC(W0, body)  — symbolic binder; body is a cons-list of output signals
SYMREF(W0)        — leaf that refers back to the enclosing binder
```

Because the conversion uses `fresh_var()` (a per-`Converter` counter), two
separately-constructed but structurally identical `DEBRUIJNREC` nodes produce
two `SYMREC` nodes with different variable symbols (e.g. `W0` vs `W2`).
`SYMREF(W0) ≠ SYMREF(W2)` as `SigId`s, so the bodies of the two `SYMREC`
groups differ at every back-reference site.  The `SYMREC` nodes themselves
therefore also differ.

**`SIGREC`/`SIGPROJ` is a legacy representation** that must not appear in the
prepared signal forest (`signal_prepare.rs` line 720 asserts this).  This plan
operates exclusively on `SYMREC`/`SYMREF` nodes.

---

## Why AFTER de_bruijn_to_sym — and AFTER a first simplify pass

### The sharing guarantee of de_bruijn_to_sym

`de_bruijn_to_sym` uses one `Converter` (shared memo + `fresh_var()` counter)
per call.  When called on the entire signal forest packed as a list (as in
`signal_prepare.rs`), any two sub-trees that share the **same `DEBRUIJNREC`
`SigId`** (i.e. identical hash-consed de Bruijn bodies) produce the **same
`SYMREC`** — the memo short-circuits on the second visit.

In that case the duplication is already eliminated by `de_bruijn_to_sym` and no
additional merge pass is needed.  The merge pass is only needed when the two
`DEBRUIJNREC` nodes have **different `SigId`s**.

### Why the de Bruijn bodies differ for fad_rec.dsp

The FAD transform applies the chain rule mechanically.  For `D(g·y)/Dg` it
emits `1·y + g·Dy` (the textbook form) without simplifying.  The manually
written `step` function writes `y_fb + g·g_fb` directly.  These are
structurally different signal sub-trees → different hash-cons `SigId`s →
different `DEBRUIJNREC` nodes → `de_bruijn_to_sym` produces two distinct
`SYMREC` even through a shared converter.

### Why AFTER simplify (first pass), not just after de_bruijn_to_sym

After `de_bruijn_to_sym` the two `SYMREC` bodies still contain the unsimplified
FAD expression `1·y + g·Dy`.  Opening them and comparing would yield different
opened-body `SigId`s (`1·y + g·Dy` ≠ `y + g·Dy`).  The isomorphism would not
be detected.

The first `simplify_signals_fastlane` reduces `1·x → x`, `0 + x → x`, etc.
After that pass both bodies become structurally identical (`y + g·Dy`).  Only
then can opening + hash-consing detect the match.

### Correct pipeline position

```
clone_forest_from(src_arena, outputs)
    ↓
de_bruijn_to_sym                       DEBRUIJNREC → SYMREC(var, body)
    ↓                                  (shared SigId → same SYMREC; else two distinct)
canonicalize_unary_rec_projections
    ↓
promote_signals_fastlane (1st)
    ↓
simplify_signals_fastlane (1st)        1·x→x, 0+x→x — FAD bodies now simplified
    ↓
[NEW] merge_isomorphic_rec_groups      opened bodies now equal → SYMREC(W2) → SYMREC(W0)
    ↓
simplify_signals_fastlane (2nd)        Proj(0,W0) − Proj(0,W0) → 0
    ↓
canonicalize_one_sample_delays
    ↓
promote_signals_fastlane (2nd)
```

This adds one extra `simplify_signals_fastlane` call after the merge.  Its cost
is bounded by the size of the substituted forest (same O(N) as the existing
simplification passes).

---

## Core concept: opening a SYMREC group

A `SYMREC(var, body_list)` node is self-referential through `SYMREF(var)` leaves
inside `body_list`.  Unlike `SIGREC`, the recursive reference is symbolic (a
named variable), so `SYMREC` itself is NOT circularly hash-consed — its SigId is
determined by its two children `var` and `body_list`, both of which are acyclic.

To compare two groups `SYMREC(W0, body0)` and `SYMREC(W2, body2)`:

1. Choose a canonical sentinel `HOLE`: a fixed `SigId` obtained by interning a
   dedicated symbol (e.g. `arena.symbol("__rec_hole__")`) once at the start of
   the pass.

2. **Open** `SYMREC(W0, body0)`: replace every `SYMREF(W0)` leaf in `body0`
   with `HOLE`, yielding `opened0`.  The traversal is memoized; other `SYMREF`
   nodes (belonging to other Rec groups) are left unchanged.

3. **Open** `SYMREC(W2, body2)` analogously, replacing `SYMREF(W2)` → `HOLE`,
   yielding `opened2`.

4. `opened0 == opened2` (same `SigId`) iff the two groups are isomorphic.

For **multi-output groups** the body is a cons-list of `k` signals.  The opened
form replaces all `SYMREF(var)` occurrences throughout the entire list; two
multi-output groups are isomorphic iff their fully-opened body lists are
identical.  No per-slot comparison is needed: hash-consing of the list structure
makes whole-list equality `O(1)`.

---

## Algorithm

### Step 1 — Collect reachable SYMREC nodes

Traverse all output signal roots depth-first (memoized `HashSet<SigId>`).
Record each `SYMREC` node encountered.  When a `SYMREF` is found as the `group`
child of a `Proj` node, follow it to its enclosing `SYMREC` and record that too.

### Step 2 — Compute the opened signature of each SYMREC

For each `SYMREC(var, body_list)` found in Step 1, traverse `body_list`
replacing `SYMREF(var)` → `HOLE`.  Cache the traversal in a per-SYMREC
`HashMap<SigId, SigId>`.  Other `SYMREC`/`SYMREF` nodes inside the body are
left as opaque leaves (no recursion into foreign Rec bodies in this step).

Result: a map `SYMREC → opened_SigId`.

### Step 3 — Group by opened signature

Build a `HashMap<SigId, Vec<SigId>>` mapping `opened_SigId → [SYMREC_1, …]`.
Groups with a single member require no action.

### Step 4 — Build the substitution map

For each group with two or more members, elect the canonical representative by
smallest `SigId` (deterministic).  Build a `HashMap<SigId, SigId>` mapping each
non-canonical `SYMREC_i → SYMREC_canon` and each `SYMREF(var_i) → SYMREF(var_canon)`.

### Step 5 — Apply substitution to the signal forest

Memoized graph walk over all output roots:
- `SYMREC(var_i, body)` where `var_i` is non-canonical → return `SYMREC_canon`
  (its body is equivalent by definition, so we reuse the canonical node directly
  without rebuilding).
- `SYMREF(var_i)` where `var_i` is non-canonical → return `SYMREF(var_canon)`.
- All other nodes: recurse into children, rebuild with `arena.intern`.

No sentinel is needed for `SYMREC` cycles: `SYMREC` is not circularly
hash-consed (`SYMREF` leaves break the cycle at the structural level).

### Step 6 — Run simplify_signals_fastlane

After substitution every reference to a formerly non-canonical group is now a
reference to `SYMREC_canon`.  A subtraction `Proj(i, W0-group) - Proj(i, W2-group)`
becomes `Proj(i, W0-group) - Proj(i, W0-group)` — same `SigId` on both sides.
The `BinOp::Sub => int(0)` rule fires and both outputs reduce to zero.

---

## Implementation plan

### New function in `crates/normalize/src/`

```rust
// normalize/src/rec_merge.rs
pub fn merge_isomorphic_symrec_groups(
    arena: &mut TreeArena,
    outputs: &[SigId],
) -> Vec<SigId>;
```

Internal helpers:
- `collect_symrec_nodes(arena, roots) -> Vec<SigId>`
- `open_symrec(arena, symrec: SigId, hole: SigId) -> SigId`  — replaces SYMREF(var) → hole
- `build_symrec_substitution(arena, symrecs: &[SigId]) -> (HashMap<SigId,SigId>, HashMap<SigId,SigId>)`  — returns (rec_map, ref_map)
- `apply_symrec_substitution(arena, roots, rec_map, ref_map) -> Vec<SigId>`

### Integration point: `crates/transform/src/signal_prepare.rs`

In `prepare_signals_for_fir_unverified`, after `promote_signals_fastlane` and
before `simplify_signals_fastlane`:

```rust
let outputs = promote_signals_fastlane(&mut arena, &sig_types_before, &outputs)
    .map_err(SignalPrepareError::Promotion)?;
// NEW
let outputs = normalize::rec_merge::merge_isomorphic_symrec_groups(&mut arena, &outputs);
let sig_types_after_merge = infer_full_types(&arena, &outputs, ui)?;
let outputs = simplify_signals_fastlane(&mut arena, &sig_types_after_merge, &outputs);
```

The function is exposed from `normalize` via `pub use rec_merge::merge_isomorphic_symrec_groups`
in `normalize/src/lib.rs`.

### Tests: `crates/normalize/src/rec_merge.rs`

| Test name | What it checks |
|---|---|
| `merge_identical_single_output_symrecs` | Two `SYMREC(W0,…)` and `SYMREC(W2,…)` with the same body after opening → unified to one canonical representative |
| `merge_identical_multi_output_symrecs` | Two 2-output SYMREC groups (primal + gradient) → unified |
| `merge_does_not_unify_distinct_symrecs` | Two Recs with different bodies → no substitution |
| `merge_followed_by_simplify_gives_zero` | Full round-trip: build `Proj(0,R1) - Proj(0,R2)` for isomorphic R1,R2 → merge → simplify → `Int(0)` |
| `merge_nested_symrec_groups` | Outer Recs referencing inner isomorphic Recs → all four unified |
| `merge_is_idempotent` | Running the pass twice produces the same output `Vec<SigId>` |
| `open_symrec_replaces_only_own_symref` | Opening `SYMREC(W0,…)` leaves `SYMREF(W2)` of a sibling group unchanged |

---

## Complexity

| Step | Cost |
|---|---|
| Collect SYMREC nodes | O(N) nodes visited |
| Open each SYMREC | O(D) per group where D = body depth |
| Group by signature | O(R) where R = number of SYMREC nodes |
| Apply substitution | O(N) nodes visited |
| **Total** | **O(N + R·D)** — linear in graph size for fixed-depth bodies |

For typical Faust DSPs, R ≤ 20 and D ≤ 50.

**Correction (2026-10-16).** The implementation did not keep foreign `SYMREC`
bodies opaque (Step 2): `open_symrec` re-interned the whole subgraph below each
body, other groups, tables and controls included, from an empty cache per
group, so the cost was O(R · N), not O(N + R·D). The faust-coopiano soundboard
(R = 2 574) spent 20 to 27 s there, 169 882 421 node visits. Keeping foreign
bodies opaque would also be wrong: a nested group may read the enclosing
variable, and its references must become holes too. The walk now stops at
subtrees in which the group's variable is not free (free-variable sets
computed once per round, `porting/MEMOIZATION.md` §2.24), which is exact when
one `SYMREC` binds the variable; a variable bound twice keeps the full walk.
Signatures, merges and created nodes are unchanged; the soundboard's pass
takes 0.056 s.

---

## Non-goals

- Semantic equivalence beyond structural isomorphism (e.g. commutativity of
  sub-expressions inside Rec bodies).
- Merging Rec groups with different output arities.
- Any change to `de_bruijn_to_sym`, the evaluator, or the propagation / FAD
  transform.
- Any change to code generation or FIR lowering.

---

## Reference

- `crates/transform/src/signal_prepare.rs` — `prepare_signals_for_fir_unverified`,
  the integration site; also `canonicalize_unary_rec_projections` for the
  `SYMREC` traversal pattern to reuse.
- `crates/tlib/src/recursion.rs` — `de_bruijn_to_sym` / `Converter::convert`;
  shows why two separate `DEBRUIJNREC` nodes always produce different `SYMREC`
  variable names.
- `crates/normalize/src/simplify.rs` — `sig_map` memoization pattern; commit
  `215b688` for the `BinOp::Sub => int(0)` rule this pass activates.
- `fad_rec.dsp` — motivating test case (manual FAD vs `fad()` macro).
