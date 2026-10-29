# Vector Shared Root Loop Cycle Fix Plan

Date: 2026-10-29
Status: complete (Z1 tests, Z2 fix and qualification landed 2026-10-29)
Scope: `crates/transform/src/signal_fir/vector/plan/build.rs` (plan producer)
Trigger: found while reducing `phaser_flanger.dsp` for the stdlib-free
`fused_serial_groups_survive_dead_pure_root_removal` test

## 1. Defect

Under `-vec`, programs in which an output returns a value that a recursion
also reads, next to an output that consumes that recursion, fall back to
scalar with `Fallback(VectorPlan)`, "epoch 0 induced graph has a cycle".
Stdlib-free reproducers:

```
process = _,_ : \(x,y).(x + (y : +~*(0.9)), y);
process = _,_ : \(x,y).(y, x + (y : +~*(0.9)));
process = _,_ : \(x,y).(attach(x, y : +~*(0.9) : vbargraph("l",0,2)), y);
```

`process = _ <: _, (+~*(0.9));` is vectorized: there the recursion's output
is its own loop's root and nothing in the root loop reads it. The fallback is
safe (the independent plan verifier rejects the plan), so this is a missed
vectorization, not a miscompile. No program of `tests/` or `examples/` is
affected (0 of 601 scanned on 2026-10-29); the shape is a common dry/wet
output.

## 2. Root cause

P4.4 gives every `Inline` sample root one shared root loop
(`allocate_loops`, `root_loop`). `place_signals` promotes such a root to
`Owned(root_loop)`, so every other loop that uses it reads it from the root
loop through a transport. In the first reproducer:

- loop 0 (root loop) owns `x + rec` and `y`;
- loop 1 (`Recursive`) reads `y`, transported from loop 0;
- loop 0 reads `rec`, transported from loop 1.

Loops 0 and 1 depend on each other. C++ `VectorCompiler::compileMultiSignal`
opens one loop per output, so it never puts `y` and `x + rec` in one loop.

## 3. Decision

Targeted correction (user decision, 2026-10-29), not C++ per-output loops:
keep the shared root loop, and move only the signals of that loop that close
a cycle into loops of their own. The `-vec` emission of every program certified
today stays the same; only programs that now fall back change. One loop per
output (C++ parity) was rejected because it changes the emission of most
multi-output programs for no correctness gain.

## 4. Correction (Z2)

1. `place_signals` takes a set of *relocated* signals. Each gets a fresh
   loop and is pre-seeded `Owned` in it, exactly like the separated signals
   of `allocate_loops`; the traversal then executes it, and its inline
   children, in that loop.
2. `build_vector_plan` runs placement and edge derivation with an empty set.
   A signal `S` owned by the root loop closes a cycle when a cross-loop use
   carries `S` from the root loop to a loop `C` that the root loop depends
   on, transitively, through the data and effect edges
   (`cycle_closing_signals`). If any exists, the set grows and placement and
   edge derivation run again, until none is left. The set only grows and is
   bounded by the number of signals.
3. The first run is the run every certified program does today, so the extra
   work is paid only by programs that would otherwise fall back.

The relocated signal is not only a promoted root: the root loop also owns
every non-duplicable signal first visited from its roots. In
`fl(d) = _ <: _, @(d) : +; process = _ <: fl(5), fl(7) : \(x,y).(x + ((x+y) : +~*(0.9)), y);`
the cycle closes through the delay read `s@5`, which the first output reads
first and the recursion fed by `x+y` reads back. A version restricted to
promoted roots left that program cyclic. A relocated loop's kind is decided
by `build_loop_records` as for any other loop (`Vectorizable` for a `Vect`,
sample-reorderable root).

## 5. Independence

The checker is unchanged: `verify_vector_plan` recomputes acyclicity,
ownership, transports and effect order from the plan alone, and it is what
rejects the base. Evidence that the new plans compute the right values comes
from outside the plan: scalar vs `-vec` bit-exactness.

## 6. Tests (Z1, written first, failing on the base)

- `crates/compiler/tests/vector_mode.rs`: the three reproducers and the
  delay-read one, `Certified` and bit-exact against scalar over a ramp
  longer than their state, all strategies, both loop variants, `-vs` 32 and
  24.
- `crates/transform/src/signal_fir/vector/plan/tests.rs`: the first
  reproducer built with `SigBuilder`; the plan verifies, the input root and
  the shared root are in different loops, and the plan is acyclic.
- Mutation: disabling the relocation (an empty set every time) must fail
  these tests.

## 7. Qualification

- Base-vs-new `-vec` emission over the `.dsp` of `tests/` and `examples/`:
  no emission changes (no corpus program hits the cycle).
- `vector-interp-opt-check`, workspace tests, clippy, fmt, the five
  structure gates, compile-cost A/B by hand on the basket.

## 7bis. Results

- Z1: both tests fail on the base with `EpochNotAcyclic`; with the
  relocation disabled they fail again (`Fallback(VectorPlan)`).
- The 35 reproducers collected while reducing are certified. The four in the
  test are bit-exact against scalar, all strategies, both loop variants,
  `-vs` 32 and 24.
- Base-vs-new `-vec` emission over the 632 `.dsp` of `tests/` and
  `examples/`: 594 identical, 0 changed, 38 failing on both binaries.
- `vector-interp-opt-check` (40 traces), workspace tests, clippy, fmt, the
  five structure gates; compile cost new/base 0.985-1.003 on the basket.

## 8. Remaining risk

Only the shared root loop is examined. A cycle through another loop (an
island or a vectorizable loop owning a signal it exports upstream) still
falls back, safely; none was found in the corpus or in the reproducers.
