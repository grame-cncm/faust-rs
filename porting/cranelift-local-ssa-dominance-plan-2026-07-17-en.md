# Cranelift Local SSA Dominance Plan

Date: 2026-07-17
Status: complete; CR1-CR2 implemented and qualified on 2026-07-18
Scope: cross-block FIR stack locals in the Cranelift `compute` lowering

## 1. Objective and baseline

`impulse_cranelift` fails factory creation on `dbmeter` and `vumeter` in every
vector mode with one CLIF verifier error:

```
inst633 store.f64 v380, v582: uses value v380 from non-dominating inst403
```

The failure predates the E0-E1 stream (a build at `2d0a2a49` fails
identically) and was exposed when UI-bearing DSPs began certifying as real
vector modules; no Cranelift impulse run has covered them since the P7 matrix.
`bargraph`, `mixer`, `spectral_level`, `karplus`, and `zita_rev1` pass: the
failing pair is exactly the lockstep register-carry shape introduced by
"Carry lockstep delay-one state in registers".

## 2. Root cause

The evidence chain, read off the full function dump
(`FAUST_RS_CLIF_DUMP`, landed in `ad2deaac`) against the C++ print of the same
FIR:

- `dbmeter`'s struct is `fSampleRate` at 0, eight `fVbargraph` zones at
  8..64, eight `vlock_*_state` persistents at 72..128.
- The zone stores (in-loop, values loaded from transport stack slots) verify
  fine. The failing stores are the eight persist saves to `dsp+72..128` in the
  chunk-tail block: FIR
  `StoreVar(vlock_*_state, Struct, LoadVar(vlock_*_local, Stack))`.
- `vlock_*_local` is a Stack scalar declared at driver scope, written once per
  sample inside the fused lockstep loop, and read once at the chunk tail -
  C-mutable-variable semantics that every C-family backend prints directly.
- The Cranelift lowering models Stack locals as raw SSA values in a
  name-to-value map. `lower_store_var_local`'s own contract states the
  assumption: it "mutates the name->value mapping only; it does not emit
  memory traffic because stack locals in the current subset are modelled as
  SSA values/pointers in the lowering environment." The chunk-tail `LoadVar`
  therefore returns the last in-loop value (`v380`, the envelope's fmax result
  defined in the loop body block), which does not dominate the tail block.

The assumption was sound while no certified FIR read a Stack scalar across
CLIF blocks; the lockstep register-carry pattern is the first shape that does.

## 3. Fix design

Model FIR `Stack`/`Loop` scalar locals as Cranelift `Variable`s:

- `DeclareVar(Stack|Loop, scalar)` declares one `Variable` with the CLIF type
  derived from the FIR type and `def_var`s its initializer (or a typed zero
  when absent, preserving current semantics).
- `StoreVar(Stack|Loop)` becomes `def_var`; `LoadVar` becomes `use_var`.
  `FunctionBuilder` then inserts block parameters wherever a value crosses a
  block boundary - the canonical Cranelift SSA construction, correct by
  construction for every current and future FIR shape, not only the lockstep
  one.
- Pointer-shaped locals (stack arrays backing transports and chunk buffers)
  keep their explicit stack slots. Their `stack_addr` is pure and constant;
  the lowering must either emit it in the entry block or rematerialize it at
  each use so no address value can be block-local. Which of the two the
  current code does is established in CR1 before any change; if addresses are
  already entry-block, they are untouched.
- `Struct`, `Static`, and `Global` accesses keep their memory semantics
  unchanged. No FIR, certificate, or C-family emitter change of any kind.

## 4. Validation obligations

This is a backend fix under an unchanged FIR contract, so the producer/checker
apparatus of the vector phases does not apply; the independent evidence is the
CLIF verifier - the checker that caught the bug - plus the native oracle:

- CR.V1: a focused unit fixture building a `compute` whose FIR writes a Stack
  scalar inside a loop and reads it after the loop; factory creation must
  succeed and the verifier report stay clean. This fails before the fix by
  construction.
- CR.V2: the 16-case Cranelift impulse matrix for `dbmeter` and `vumeter` at
  `-lv 0/1 x -ss 0..3`, `.ir` deleted first and comparison counts asserted -
  the harness reports green from cached outputs otherwise.
- CR.V3: no collateral regression: the full scalar Cranelift impulse suite,
  plus a Cranelift vector sweep over the certified corpus at `-lv 0 -ss 0`,
  must hold their current pass set.
- CR.V4: `cargo test -p codegen -p cranelift-ffi`, `cargo fmt`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and the workspace
  test suite.

Numeric parity is the oracle's job: `use_var`/`def_var` changes value routing,
not arithmetic, and the 60,000-frame comparisons are the arbiter that no
routing change altered a sample.

## 5. Rollout

### CR0 - plan

This document plus the diagnostics already landed in `ad2deaac`. No compiler
behavior change.

### CR1 - SSA variables for scalar locals

The `Variable` model for scalar Stack/Loop locals, the pointer-local address
placement audit, and CR.V1. Lands only with CR.V1 green.

### CR2 - harness qualification

CR.V2 through CR.V4, plus the journal entry recording the measured pass set.

## 6. Risks and mitigations

- `use_var` inserts block parameters; on this corpus the affected values are
  a handful of `f64` carries per lockstep bundle, so code-quality impact is
  noise. The compile-budget basket does not exercise Cranelift; no new gate is
  added for it.
- `Variable`s require a declared type per local. FIR `DeclareVar` carries it;
  a local whose type the lowering cannot map stays a hard error, as today.
- Sealing order is unchanged: the existing loop lowerings seal header/body
  after their predecessors are complete, which is exactly what the
  `FunctionBuilder` SSA construction requires.
- If the pointer-local audit finds block-local `stack_addr` values, fixing
  them is in scope for CR1; leaving them would keep a second instance of the
  same bug class latent.
