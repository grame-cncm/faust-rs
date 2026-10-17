# W7: reusing locals in `compute` (FIR pass) — design note

Date: 2026-10-17. Parent plan: `wasm-compute-speed-analysis-and-plan-2026-10-16-en.md`
(W7 is new). Sibling pass: W2/W3 (`wasm-w2-w3-sinking-forwarding-design-2026-10-17-en.md`).
Decisions taken by the user on 2026-10-17: fix it in faust-rs, **as a FIR pass**
(not in the wasm emitter), with an independent check.

## 0. Problem

faust-coopiano's web page is silent in Safari (macOS Tahoe): the AudioWorklet's
`process()` throws. Reproduced with JavaScriptCore (`jsc`, Safari's engine):
`RangeError: Maximum call stack size exceeded` in the wasm `compute` once the
thread's stack is 512 KB (the default of a secondary thread on macOS), while
1 MB is enough. `compute` declares one wasm local per FIR stack variable:
37,720 locals for the voice, 19,391 and 22,326 for the two bodies.
JavaScriptCore's baseline tier gives each local its own stack slot (and zeroes
them all on entry), so the frame alone is 300 KB for the voice. Measured stack
need: voice 512–768 KB, bodies 384–512 KB. V8 (Chrome) turns locals into SSA
values and is not affected.

## 1. What changes

A FIR-to-FIR pass on `compute`, run by `fir::schedule::schedule_compute` after
W3 and W2 (so for the interp, Cranelift and wasm backends; C and C++ unchanged).
Stack scalars whose lifetimes do not overlap share one variable:

- a *slot* `fReuseN` (double), `fReusefN` (float), `iReuseN` (int),
  `lReuseN` (int64) is declared once, without initializer, at the top of
  `compute`;
- each `DeclareVar x = e` of a variable given slot `s` becomes `StoreVar s = e`
  (`DeclareVar x` without initializer becomes `StoreVar s = 0` of its type, the
  value the JIT backends already give it);
- each `LoadVar x` and `StoreVar x` becomes `LoadVar s` / `StoreVar s`.

The FIR stays valid (no name is declared twice) and every backend reads it
as before. The arithmetic and the values are unchanged: outputs must stay
bit-identical. Expected effect: thousands of locals become hundreds, so
Safari's frame fits; the interpreter's heap shrinks the same way.

## 2. Producer (`fir::schedule::coalesce`)

Candidates: stack (`AccessType::Stack`) scalar variables of type `Int32`,
`Int64`, `Float32` or `Float64` declared in `compute`, except names starting
with `input`/`output` (the interpreter's buffer pseudo-variables), names that
look like slots, and any variable mentioned by a node the pass does not
rewrite (`TeeVar`, `LoadVarAddress`, any unknown statement) or declared with
two types. Loop variables are never candidates.

Live interval: a walk numbers the statements in program order (an `if`'s arms
one after the other; a loop's header at its start). A variable lives from its
first declaration to its last mention. A loop extends it to the loop's end
when the variable is declared before the loop and mentioned inside, or when,
inside the loop, its first mention is not a declaration (a value carried
around the back edge). Linear scan per type, in order of start, reuses a slot
whose last interval ended strictly before.

## 3. Independent checker (`fir::schedule::check`)

Walks the original body and the rewritten one in parallel, with its own
abstract state: for each slot, the variable whose value it holds (`owner`),
or *unknown*. It requires:

1. the rewritten body is the slot declarations (fresh names, declared once,
   scalar, no initializer) followed by the original statements one to one;
2. the renaming is a function: a variable always maps to the same slot, of its
   own type; a variable that is not renamed keeps its name;
3. a `DeclareVar x` corresponds to `StoreVar s` of the same value (or of 0);
   writes set `owner[s] = x`;
4. **every read of `x` through `s` happens where `owner[s] = x` on every
   path.** Branches join (`x` only if every arm agrees); loops are iterated to
   a fixed point from their entry state, then checked once.

Point 4 is the whole correctness argument: every write of `x` is a write of
`s`, so when the last write of `s` is a write of `x`, it is the last write of
`x`, and the read sees the original value. It does not use the producer's
intervals, numbering or loop rule.

Fail closed: a rejection drops W7 for the function (W2/W3 stay) and asserts in
debug builds.

Mutations the checker must reject (unit tests): two variables live at once on
one slot; a variable declared before a loop sharing a slot with one declared
in it; a value carried around the back edge, overwritten by a slot mate; a
slot holding different variables after an `if`'s arms, read after it; a
renaming that is not a function; a slot of another type; a slot name already
used by the program; a lost or extra statement.

## 4. Qualification

- Unit tests of producer, checker and mutations (`fir` crate);
  `crates/compiler/tests/fir_schedule.rs` (interpreter outputs bit-identical,
  module verified, idempotent).
- Debug corpus: every `.dsp` of `tests/` and `examples/` scheduled with no
  rejection, scheduled module verified.
- Impulse tests of interp, Cranelift and wasm: every `.ir` byte-identical.
- faust-coopiano: locals of `compute` before/after; the three modules under
  `jsc` at 512 KB of stack, and the processor with its 8 voices; then Safari.
- Speed: wasm (V8) slope method on the bank, the voice and the bodies, and
  JavaScriptCore; Cranelift `--time`; compile cost of the pass.

Results (2026-10-17), as run:

- 5 producer tests and 7 mutation tests in `fir` (`schedule/tests_reuse.rs`),
  each mutation rejected for its own reason; `fir_schedule.rs` unchanged and
  passing (now through W7);
- debug corpus: 516 programs, 385 changed, no rejection, all verified;
- impulse tests: 93 `.ir` per backend, byte-identical to the parent's;
- faust-coopiano: `compute` locals voice 37,720 → 2,177, bodies 22,326 → 4,410
  and 19,391 → 2,946; under `jsc` the modules and the 8-voice processor run in
  256 KB of stack, with the same peaks to the bit;
- speed and compile cost: parent plan §10.

## 5. Mapping and registry

No public API change (`schedule_compute` gains a stage). Generated wasm,
interpreter bytecode and Cranelift code reuse variables where Faust C++ keeps
one per value: registry entry DIFF-BACK-009 is extended.
