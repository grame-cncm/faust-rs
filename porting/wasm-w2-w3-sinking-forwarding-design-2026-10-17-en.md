# W2/W3: sinking reads and forwarding stores in `compute` — design note

Date: 2026-10-17. Parent plan: `wasm-compute-speed-analysis-and-plan-2026-10-16-en.md`
(sections 3 C2/C3 and 4 W2/W3). Decisions taken by the user on 2026-10-17:

- **D2: a FIR-to-FIR pass** in the `fir` crate (`fir::schedule`), so the FIR
  verifier checks its output and the tests can run both versions;
- **applied by the interp, Cranelift and wasm backends only.** C, C++ and every
  other text backend keep today's FIR and output (no golden churn);
- **D3: W2 and W3 now.**

## 1. What changes

Only `compute`, and only the order of its statements and some of its loads.
The same arithmetic runs on the same values, so outputs must stay
bit-identical.

- **W3, forwarding (first).** After a store of a scalar to a constant struct
  address (`fRec0[0] = fRecBody6;`, `fState = fTemp3;`), a later load of the
  same address in the same straight-line region (`fRec0[1] = fRec0[0];`,
  `output0[i0] = fRec0[0] + …`) reads the stored value instead, as long as
  nothing in between may write the address. Only values that are already a
  local assigned once in `compute`, or a constant, are forwarded, so no
  variable is created. This removes the store→load round trips of C3.
- **W2, sinking (second).** In every `Block` of `compute`, a *movable*
  declaration is emitted later than written: just before the first statement
  that reads the local it defines, or before the first statement that may
  write something it reads, whichever comes first. `double fTemp36 =
  fRec2[1];` at the top of the sample loop then sits next to its use instead of
  23,000 statements earlier. W3 runs first because each forwarded load is one
  memory read fewer that could block a declaration from sinking.

## 2. Alias model (shared vocabulary, two implementations)

Addresses:

- `Field(x)`: struct or static scalar `x`, or the whole array `x`.
- `Elem(x, k)`: element `k` (a FIR `Int32` constant) of struct/static array `x`.
- `Local(x)`: a stack, loop or argument scalar local.
- `Stack`: every stack table, the audio buffers included. Inputs and outputs
  may be the same host buffer (in-place processing), so they are one region.
- `All`: anything (an unknown statement or an impure call).

Conflicts: `Field(x)` meets `Field(x)` and every `Elem(x, _)`; `Elem(x, k)`
meets `Elem(x, k)` only; `Stack` meets `Stack`; `All` meets everything. Distinct
names never overlap: the backends give struct fields, static tables and local
arrays disjoint storage.

Pure values: constants, loads of the kinds above, `BinOp`, `Neg`, `Cast`,
`Bitcast`, `Select2`, and `FunCall` of a mathematical function
(`FirMathOp::from_symbol`, `abs`, `max_i`, `min_i`, the hyperbolic functions,
`copysign`, `isnan`, `isinf`). Anything else, notably a foreign function
(`ffunction`), `TeeVar` or `LoadVarAddress`, is impure: never moved, and its
statement writes `All`.

## 3. W3 producer (`fir::schedule::forward`)

A walk of `compute`'s body keeps `available: Addr → value` for the current
region:

- a store to `Field(x)` (`StoreVar` struct/static) or `Elem(x, k)` first
  invalidates the entries it conflicts with, then, if its value is a local read
  `LoadVar(y)` with `y` assigned once in `compute`, or a constant, records
  `available[addr] = value`;
- a load of an available address, with the same type as the value, is
  replaced by the value;
- any other statement invalidates the entries its writes conflict with (`All`
  clears the map);
- an `If`/`Control`/`Switch` arm starts from a copy of the map; its own entries
  are dropped at the end; afterwards the entries written in any arm are
  invalidated;
- a loop body starts from an empty map; afterwards the entries written in the
  loop are invalidated.

## 4. W2 producer (`fir::schedule::sink`)

In a `Block`, item `i` is movable when it is `DeclareVar { access: Stack,
init: Some(e) }` of a non-array local `x`, `e` is pure, and no other item of
the block's subtree writes `x`. Processing items from last to first,
`anchor(i)` is the minimum of `anchor(u)` over the items `u > i` that read `x`
(a fixed item is its own anchor) and the first `j > i` that writes an address
`e` reads. The item then stops `SLACK` (8) *solid* items before that anchor —
solid meaning neither movable nor inert (a label, an empty statement). With no
anchor, or when the result passes no solid item, the item stays. The order
walks the fixed items and emits before each the movable items anchored to it,
in their original order. Per-address sorted writer lists make each query a
binary search: `O(n log n)` per block. Nested blocks are scheduled the same
way, innermost first.

Why the slack: sinking a load right next to its use put its latency on the
critical path of serial code. With no slack, the faust-coopiano voice (whose
hammer is solved by Newton iterations) took 25 % more cycles per sample in V8
while executing 9 % fewer instructions; the light body gained 33 %. A slack of
4, 8 or 16 statements removes the voice's loss and keeps the bodies' gain,
within the measurement noise; 8 is used. Passing only movable or inert items
gains nothing and made a second run swap such items back, so it is not a
move.

## 5. Independent checker (`fir::schedule::check`)

Separate code with its own walker, purity rule and conflict test.

**W3.** Walks the original body (source store) and the forwarded body
(destination store) in parallel. Statements correspond one to one; values are
compared structurally. Where the original has a struct load of `A` and the
forwarded body something else, the checker requires that its own
`known[A]` (built from the stores it has walked, invalidated by conflicting
writes, copied into arms and emptied in loops) is that same value, and that
the value is a single-assigned local or a constant. Any other difference is a
rejection.

**W2.** For each block, given its items and the chosen order:

1. the order is a permutation;
2. the items the checker classifies as fixed keep their relative order;
3. an item emitted after a fixed item that originally followed it is movable
   by the checker's rule, its local has no other writer, no reader of its
   local precedes it, and no item it moved past writes an address its
   initializer reads;
4. no item moves earlier than a fixed item that preceded it.

The checker also reassembles the scheduled tree from the forwarded tree and the
orders, and requires it to be the produced tree.

**Fail closed.** A rejected W3 drops W3 for the function; a rejected W2
drops W2 for the function. Debug builds assert, so the test suites cannot pass
with a rejection.

**Mutations the checker must reject** (unit tests): a read moved past a store
to its element, to its whole array, past an `All` statement, or past a store to
a local it reads; a declaration moved after its reader; an impure declaration
moved; a declaration whose local is assigned again, moved; two fixed
statements swapped; an item moved earlier; an item dropped or duplicated; a
forward across a conflicting store, from an arm to after it, into a loop, of a
different element, of a local assigned twice.

## 6. Qualification

- Unit tests of the producers, the checker and the mutations (`fir` crate).
- The FIR verifier on the scheduled module of every `.dsp` of `tests/` and
  `examples/` (`--dump-fir-verify` path, interp, Cranelift, wasm).
- Bit-identical outputs, before and after, for every `.dsp` of `tests/` and
  `examples/`: interp and Cranelift through faustprobe's `--ref`, wasm through
  the impulse-test runner, against the parent commit's binaries.
- The impulse tests of the three backends (`tests/impulse-tests`).
- faust-coopiano's `make check` and `make web-check`.
- Speed: the slope method (parent plan §8) on a resonator bank and on
  faust-coopiano's three wasm modules; faustprobe `--time` for Cranelift.
- Compile cost: an A/B by hand (`compile-budget-check` cannot run here), on the
  imaged body and the voice for the three backends.

Results (2026-10-17), as run:

- unit tests: 28 in `fir` plus 2 consistency tests of the two alias
  implementations; `crates/compiler/tests/fir_schedule.rs` (interpreter outputs
  bit-identical with and without the pass on three programs, verified,
  idempotent);
- debug corpus (`tests/`, `examples/`): 516 programs compile to FIR, 97 are
  changed by the pass, no checker rejection, every scheduled module verified;
- bit identity: the impulse tests of interp, Cranelift and wasm (every `.ir`
  file byte-identical to the parent commit's) stand for the corpus-wide
  comparison; faustprobe `--ref` at zero tolerance on the bank and the imaged
  body; faust-coopiano's three wasm modules compared sample for sample
  (checksums) instead of `make check`/`make web-check`;
- speed and compile cost: parent plan §9 (the voice was not timed for
  compile cost).

## 7. Mapping and registry

New public API: `fir::schedule` (one entry function). Generated wasm,
interpreter bytecode and Cranelift code differ from Faust C++'s in statement
order and in some loads only; `porting/faust-rs-vs-faust-cpp-differences-en.md`
gets an entry.
