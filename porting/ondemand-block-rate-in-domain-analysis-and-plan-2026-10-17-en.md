# `ondemand`: block-rate work leaves the domain — analysis and plan

Date: 2026-10-17

Status: **implemented** on 2026-10-17 (section 8). The user's decisions:
option c, grouped per guard (D1); `downsampling` keeps the hoisting (D2); so
does external control, `-ec` (D3); coopiano's `on_change` is simplified later,
in a separate change in its own repository (D4).

Origin: point 6 of faust-coopiano's `faust-rs-issues/RAPPORT.md`. A boolean
`ondemand` whose payload reads only control-rate values computes that payload
before the sample loop, at every block, instead of when its clock fires. coopiano
works around it with `on_change`, which feeds the payload an audio-rate copy of
its input (`ba.sAndH(chg, x)`).

| Compiler | Version |
|---|---|
| faust-rs | `main-dev` at `8b6d7930` |
| Faust C++ reference | 2.84.3, `master-dev-ocpp-od-fir-2-FIR19` at `8eebea429` |
| JS engine | node 22.12.0, V8 12.4 |

All prototypes below are throwaway patches, built to separate binaries and
reverted; none is committed.

## 1. Summary

1. **Reproduced, and the same in Faust C++.** In `r13a`,
   `(m != m', m) : ondemand(heavy)` with `m` a slider puts the 15 `exp` of
   `heavy` in `fSlow1` before the loop; the guard only copies `fSlow1` into the
   hold field. The pinned C++ compiler emits the same (`fSlow5BE` before the
   loop). Upstream `master-dev` has no `ondemand` work beyond the pinned branch,
   so a change is a deliberate difference from C++.
2. **It contradicts the documentation.** `docs/ondemand-note-en.md` §4 says of a
   0/1 clock that "the body genuinely does not execute".
3. **It costs coopiano a factor of 2.2 without the workaround.** The voice with
   `on_change` reduced to `(chg, x) : ondemand(f)` runs 44,900 instructions and
   11,700 cycles per sample in wasm, against 20,100 and 4,850 with the
   workaround.
4. **Option a (keep block-rate values in the guard, computed at each fire) is
   rejected.** It is exact, but a clock that fires every sample pays the payload
   every sample: 8,048 instructions per sample instead of 141 on a 16-term
   payload (wasm), 1,191 instead of 18 natively.
5. **Option c (compute them at the first fire of each block, behind a
   per-guard flag) gives the best of both.** On wasm: 37 instead of 299
   instructions per sample when the clock rarely fires, 110 instead of 143 when
   it fires every sample, 112 instead of 108 every 32 samples. Without its
   workaround the coopiano voice drops to 22,300 instructions and 5,000 cycles,
   within 3 % of the cycles of the workaround version. Outputs are
   bit-identical to today's on every corpus program.
6. **The flag must be per guard, not per value.** A first prototype with one
   flag and one `if` per value took 115 s instead of 12 s to compile the voice
   (W2/W3/W7 scheduling passes), and V8 rejected the module ("local count too
   large"). The grouped version compiles it in 14.3 s.

## 2. Measurements

### 2.1 Cost, three clocks

`heavy` is a 16-term (64 for `rare64`) sum of `exp`·`cos` of a slider. The
clocks: `m != m'` (`rare`, fires once), `ba.pulse(1)` (every sample),
`ba.pulse(32)`. Instructions and cycles per sample, from `/usr/bin/time -l`,
slope between two run lengths:

| DSP | today | option a | option c (grouped) |
|---|---|---|---|
| `rare64`, wasm | 299 / 51 | 40 / 17 | 37 / 13 |
| every sample, wasm | 143 / 26 | 8,048 / 1,110 | 110 / 21 |
| every 32, wasm | 108 / 19 | 297 / 48 | 112 / 25 |
| every sample, native C++ | 18 instr | 1,191 instr | 18 instr |

Natively, clang's loop-invariant code motion hoists option c's lazy
computation back out of the loop (the payload is pure and its inputs are
loop-invariant), so option c costs what today's code costs. It cannot hoist
option a's, which re-reads the slider inside the guard. V8 hoists neither.

### 2.2 coopiano voice without the workaround

The voice compiled to wasm (`-double`), W7 included, held C4:

| Build | instr/sample | cycles/sample | compile |
|---|---|---|---|
| today, with `on_change` (`sAndH`) | 20,100 | 4,850 | 10.8 s |
| today, `on_change(chg, x, f) = (chg, x) : ondemand(f)` | 44,900 | 11,700 | 10.3–12.0 s |
| option c, same program | 22,300 | 5,000 | 14.3 s, 2.9 GB |

Option c and today's code give bit-identical outputs on the same program. The
program without the workaround differs from the one with it by −259 dB of the
peak, with either compiler: block-rate and sample-rate expressions are grouped
differently.

### 2.3 Scope of the change

Over the 632 `.dsp` of `tests/` and `examples/` (`-lang cpp`), option c
changes the generated code of 16 programs, all of them `ondemand_*` impulse
tests. 500 are unchanged, and 116 fail to compile identically before and
after. Option a changes the same 16.

## 3. Cause

- **Typing.** `TempVar(x)`, the input of a domain, takes the type of `x`
  (`crates/sigtype/src/rules.rs:1249`). A slider input therefore stays `Block`,
  and so does everything the payload computes from it.
- **Placement.** `place_by_variability`
  (`crates/transform/src/signal_fir/module/core_lowering.rs:159-188`) sends
  every shared or boundary `Block` signal to the `Control` bucket: a `fSlow`
  declared before the sample loop. It does so whatever guarded region is open.

The guard itself (`clocked.rs`, `build_guarded_block`) is correct: it only
stores the hoisted value into the hold field.

## 4. Options

**a. Keep block-rate values in a boolean `ondemand` guard, as sample values.**
A few lines in `place_by_variability`. Exact, but it multiplies the payload's
cost by the fire rate (section 2.1). Rejected.

**c. Compute them at the first fire of each block** (recommended).

- **Selection.** In a boolean `ondemand` guard (`GuardShape::BoolIf`), a `Block`
  value is lazy: its variable is declared, zero-initialized, before the sample
  loop.
- **Prologue.** Its store goes into a prologue at the top of that guard:
  `if (flag == 0) { v1 = …; v2 = …; flag = 1; }`, with one flag per guard. The
  flag is a local reset to 0 at each `compute` call, so the values are computed
  at most once per block, and only if the clock fires.
- **Dependencies.** A lazy value depends only on `Konst` and `Block` values.
  Those are hoisted constants, earlier entries of the same prologue, or
  values hoisted before the loop, so the prologue order (materialization
  order) is a valid order.
- **Exactness.** A `Block` value is constant within a block, so computing it at
  the first fire gives the same bits as computing it before the loop.
- **What is unchanged.** Integer `ondemand` and `upsampling` (counted loops,
  several runs per sample) keep the hoisting, which is the cheaper choice
  there. So does `downsampling` (see D2).

The measured prototype adds a `lazy_prologues` list per open guard to
`ClockedState`, a `materialize_lazy_block` path in `place_by_variability`, and
the prologue insertion in `lower_guarded_body`. It applies to the scalar and
vector paths alike.

**Status quo, documented.** Keep C++ parity and correct
`docs/ondemand-note-en.md` to say that block-rate work in a domain runs at
every block. coopiano keeps `on_change`.

## 5. Plan for option c

1. **Implementation** (`crates/transform`):
   - the per-guard prologue: one flag per guard, created on the first lazy
     value;
   - lazy declarations in `compute`'s prelude. Under external control
     (`-ec`) the `Control` section moves to `control()`, so the lazy
     declarations must not follow it (D3);
   - Rustdoc on the provenance, and on why this departs from C++
     `generateOD`.
2. **Unit tests** (`crates/transform`, test-local DSPs, no installed
   libraries):
   - `r13a`'s shape: no `exp` before the loop, one prologue in the guard;
   - two lazy values sharing one flag;
   - a nested boolean domain gets its own prologue;
   - integer `ondemand`, `upsampling`, `downsampling` and `-ec` keep the
     hoisting;
   - a value used both inside and outside the guard stays hoisted outside.
3. **Structural check.** In every guard, each read of a lazy variable comes
   after that guard's prologue.
4. **Differential and impulse runs.**
   - Base against new on every `.dsp` of `tests/` and `examples/`, wasm,
     `-double` and `-single`, outputs bit-identical. The prototype passes this
     (1,024 identical, 5 trapping identically).
   - Impulse lanes: `cpp`, `c`, `wasm`, `interp`, `cranelift`, `cpp-vec0`,
     `cpp-vec1`. The prototype passes the 16 changed programs on `cpp`, `c`,
     `wasm`, `cpp-vec0` and `cpp-vec1`.
   - The vector certification gates, since vector code changes.
5. **Compile cost.** `compile-budget-check` (or the by-hand A/B on this
   machine), with the voice without the workaround in the comparison; the
   prototype measured +19 % on it.
6. **Records.**
   - Registry entry (DIFF-BEH-024): the generated code differs from C++ for a
     boolean `ondemand` with a block-rate payload; outputs are identical.
   - `docs/ondemand-note-en.md` §4: its promise now holds for block-rate work.
   - Journal.
7. **coopiano** (separate repository, separate change): `on_change` can
   become `(chg, x) : ondemand(f)`.

Expected gain: as measured in section 2. Risk: medium; this is a placement
change in the lowering, guarded by the bit-exact differential and the
structural check above.

## 6. Decisions for the user

- **D1, which option.**
  - c, grouped per guard (recommended): exact; never measurably slower; the
    documented behavior.
  - Status quo with corrected documentation: C++ parity.
- **D2, `downsampling`.** Its guard fires once every N samples. Hoisting wins
  when N is smaller than the block, lazy evaluation when it is larger. I propose
  keeping the hoisting for now and revisiting with a measurement.
- **D3, external control (`-ec`).** Under `-ec`, `control()` computes the
  `Control` section and `compute` reads it. Option c can either keep the
  hoisting in that mode (simplest, as the prototype does) or declare its lazy
  values in `compute` regardless. I propose the former.
- **D4, coopiano.** After option c lands, simplify `on_change`: the voice
  then costs 22,300 instead of 20,100 instructions per sample (cycles within
  3 %). Or keep it, which is 10 % cheaper in instructions.

## 7. Side finding: W7's checker on many `if`s

The per-value prototype exposed a scaling limit in W7
(`8b6d7930`): `fir::schedule::check_reuse::join` clones a map of live locals at
each `if`, so its cost grows with the number of `if`s times the number of
locals. Grouping avoids it here. A program with thousands of guarded blocks
would hit it anyway; it is worth a follow-up in the W7 design note.

## 8. Implementation

Decided as proposed (status line). The code:

- `crates/transform/src/signal_fir/module/clocked.rs`: `LazyPrologue` (one per
  open block, parallel to `open_domains`), `lazy_block_slot`,
  `materialize_lazy_block`, `emit_lazy_prologue`, and the prologue insertion in
  `lower_guarded_body`. The module documentation gives the contract and the
  departure from C++ `generateOD`.
- `crates/transform/src/signal_fir/module/core_lowering.rs`:
  `place_by_variability` sends a `Block` value to the lazy prologue when
  `lazy_block_slot` names a block, and to the `Control` bucket otherwise.
- `crates/transform/src/signal_fir/placement.rs`:
  `guard_exclusive_block_signals`, a pre-pass run for clocked programs under
  internal control only.

### 8.1 Two refinements over the prototype

Both were found by measuring the coopiano voice without its workaround.

1. **Values only the prologue reads are its locals.** The prototype declared
   every lazy value, zeroed, before the loop. The voice has 36,049 of them in
   48 guards, and only 2,513 are read after their prologue. Live across the
   sample loop, the others defeat W7's local reuse:
   - wasm `compute` had 38,616 locals instead of 5,013;
   - compilation took 14.5–15.5 s instead of 10.1 s. The wasm backend's local
     collection (`collect_compute_locals`) is quadratic in the number of
     locals.

   A value that the rest of the guard reads is still declared before the loop,
   since a later fire of the block reads what the first one stored. Any other
   value is now declared in the prologue. Result: 5,210 locals, 10.3–10.7 s.
2. **A value also needed outside the guard stays hoisted.** Plan step 2 asked
   for this test, and the prototype failed it. A `Block` value read by the
   payload and by code outside the guard was computed twice in a block where
   the clock fires: once in the prologue, and once hoisted for the outside
   reader. main-dev computes it once, because the hoisted copies merge.
   `guard_exclusive_block_signals` marks the `Block` signals that only the
   payload of one clocked block reaches (owner analysis over the DAG, at most
   two owner changes per signal), and only those are lazy. As a result,
   `ondemand_10_adsr_layers` is back to main-dev's code, and 15 programs change
   instead of 16.

### 8.2 Tests

`crates/compiler/tests/ondemand_lazy_block.rs`: eight tests with test-local
DSPs. They cover:

- `r13a`'s shape;
- several values sharing one flag, with the split between values kept before
  the loop and prologue locals;
- nested guards;
- a value also needed outside the guard;
- integer `ondemand`, `downsampling`, `upsampling` and `-ec` keeping the
  hoisting;
- `-vec -lv 0` and `-lv 1`;
- the interpreter's output equal to the hoisted program's.

A FIR walker checks the structural contract (plan step 3) in each FIR test:

- every read of a lazy value comes after its computation, on a path through
  its own guard's prologue;
- a prologue local is read only inside its prologue;
- a prologue is the first statement of its guard, and each guard has at most
  one.

Mutations, each caught by the tests:

| Mutation | Caught by |
|---|---|
| no lazy arm in `place_by_variability` | four tests |
| prologue appended at the end of the guard | the walker: read before the prologue |
| every value made a prologue local | the walker: read before the prologue, four tests |
| every value declared before the loop | the kept/local split |
| no exclusivity check | the outside-reader test: `(3, 3)` `exp` instead of `(3, 0)` |

### 8.3 Validation

- Local gates: `cargo fmt`, `clippy -D warnings`, `cargo test --workspace
  --all-targets`, the five structure gates and `golden-check` (results in the
  journal entry).
- Vector gates (plan step 4): `vector-coverage-check` keeps its 1,568
  certified mode/DSP pairs (16 modes, 133 corpus DSPs), and
  `vector-interp-opt-check` passes. Both were run with the local
  faustlibraries checkout, not the revision CI pins.
- Differential, base `main-dev` against new, over the 632 `.dsp` files of
  `tests/` and `examples/`:
  - C++ text: 15 programs changed, all `ondemand_*` impulse tests; 501
    unchanged; 116 fail identically before and after;
  - wasm, `-double` and `-single`: 1,024 bit-identical, 5 trapping
    identically.
- Impulse lanes on the 21 `ondemand_*` programs, against the C++ references:
  21/21 on `cpp`, `c`, `wasm`, `interp`, `cranelift`, `cpp-vec0` and
  `cpp-vec1`.
- Compile cost, by hand (`compile-budget-check` refuses to run on this machine):
  - the budget basket and three `ondemand_*` programs, best of five, new over
    base: ratios 0.99–1.02;
  - APF first read 1.16 on an 8 ms compile; it has no clocked block, so the
    new pass does not run there. Its medians of 30 runs give 0.97 and 1.01.
- coopiano voice without its workaround, held C4, wasm `-double`:

| Build | instr/sample | cycles/sample | compile | locals |
|---|---|---|---|---|
| main-dev, with `on_change` (`sAndH`) | 20,150 | 4,880–4,910 | 10.8 s | 2,956 |
| main-dev, without | 44,900 | 11,400–11,700 | 10.4–10.6 s | 5,013 |
| this change, without | 22,270 | 4,990–5,080 | 10.6–10.7 s | 5,210 |

The outputs of the last two rows are bit-identical.

