# Vector mode FIR model hardening plan

**Date**: 2026-07-10
**Branch**: `ondemand-vec-fad-synthesis`
**Status**: proposed
**Scope**: `crates/transform/src/signal_fir/loop_graph.rs`,
`crates/transform/src/signal_fir/module/build.rs`, FIR value traversal helpers,
compiler vector-mode tests, and `tests/impulse-tests`.

Related:

- [`vector-mode-analysis-port-plan-2026-06-10-en.md`](vector-mode-analysis-port-plan-2026-06-10-en.md)
- [`vector-mode-loop-separation-plan-2026-07-09-en.md`](vector-mode-loop-separation-plan-2026-07-09-en.md)
- [`fir-pattern-rewrite-engine-plan-en.md`](fir-pattern-rewrite-engine-plan-en.md)

---

## 1. Problem statement

The `-vec` infrastructure and loop-separation substrate are in place, but the
first full impulse runs exposed unsound FIR-level recursive splitting. The
failure is not specific to a backend or to `-lv 0` versus `-lv 1`: the current
split sometimes moves a state-free-looking tail after a serial recursive core
even though the tail still reads state hidden inside a FIR value expression.

Two concrete failure shapes found by `tests/impulse-tests`:

- `APF.dsp`: the hoisted output tail reads recursive table state through
  `LoadTable(fRec, Struct, ...)`, but dependency collection only saw the index
  expression. The tail then observes the chunk-final table state instead of the
  per-sample state.
- `sound.dsp`: the soundfile buffer index depends on a recursive temporary.
  Dependency collection and load rewriting must descend into
  `LoadSoundfileBuffer` operands; otherwise the split can either miss the
  dependency or generate a tail that still references an out-of-scope serial
  local.

The root cause is that `partition_recursive_body`, `collect_var_loads`, and
`rewrite_var_loads` maintain hand-written, partial knowledge of FIR value
shapes. Any missed read or missed rewrite can make the split incorrect.

## 2. Why harden FIR first instead of porting the C++ signal-level vectorizer now

Faust C++ makes vector-mode decisions while compiling signals. `VectorCompiler`
and `DAGInstructionsCompiler` attach vector properties to signal expressions,
materialize cross-loop buffers, and build the loop DAG before the computation is
fully flattened.

That model is a long-term parity target, but it is too large for the immediate
bug fix:

- it would require porting the C++ signal-level vector property model, delay
  layouts, buffer naming, loop-stack behavior, and memo-hit dependency tracking;
- it would touch the `signal -> FIR` lowering boundary, where faust-rs currently
  keeps scalar and vector behavior sharing one path;
- it would not remove the need for a correct FIR traversal API, because later
  FIR passes and verifiers still need complete read/write/rewrite semantics.

The short-term fix should therefore keep the current faust-rs architecture and
make FIR-level splitting conservative and total. This gives a correct `-vec`
baseline quickly. A later signal-level vectorizer can then be evaluated as a
performance/parity improvement rather than as an emergency correctness rewrite.

## 3. Correctness rule

A recursive-body split is valid only if every statement moved to a vectorizable
tail can be proven independent of loop-carried state after boundary buffering.

Concretely:

- every FIR value node in a candidate tail must be traversed for reads;
- every cross-boundary temporary read must be rewritten to a chunk-buffer load;
- every unrewritable cross-boundary read forces fallback to one serial loop;
- direct reads or writes of persistent `Struct` state in the tail force fallback,
  unless the value is explicitly materialized by a proven buffer protocol;
- unsupported statement/value forms force fallback, not speculative hoisting.

This rule intentionally allows missed optimization. It does not allow missed
dependencies.

## 4. Implementation plan

### H1 — Add total FIR value traversal helpers

Introduce a single shared traversal/mapping surface for FIR value nodes used by
the vector split:

- `visit_fir_value_reads(store, id, visitor)` for read/effect collection;
- `map_fir_value(store, builder, id, mapper)` or an equivalent helper for
  recursive rebuilding.

The first version can live near `loop_graph.rs` if that keeps the change small,
but the API should be shaped so it can move into `crates/fir` or a local FIR
utility module once other passes need it.

Minimum value forms to cover in the first landing:

- `LoadVar` and `LoadVarAddress`;
- `LoadTable` including both table name/access and index;
- `LoadSoundfileLength`, `LoadSoundfileRate`, `LoadSoundfileBuffer`;
- `ValueArray`;
- unary, binary, cast, select, and foreign/function-call arguments;
- every existing FIR value variant matched by `match_fir`.

Pass criterion: a unit test fails if a new FIR value variant is not classified as
visited, ignored-by-design, or fallback-only.

### H2 — Make dependency collection effect-based

Replace `collect_var_loads` with an effect collector built on H1:

- classify reads by storage access (`Stack`, `Loop`, `Struct`, input/output
  arrays, soundfile tables, etc.);
- keep table loads as reads of the table storage plus reads of their index;
- keep soundfile loads as reads of their operand expressions;
- record direct persistent-state reads in candidate tails.

Pass criterion: APF-like `LoadTable(Struct)` tails are rejected or fully
bufferized; they must never be hoisted while still reading mutable struct state.

### H3 — Make tail rewriting symmetric with dependency collection

Rebuild tail statements with a rewriter that descends into exactly the same FIR
value forms as the collector:

- rewrite boundary `LoadVar(Stack)` reads to `ChunkBuffer` loads;
- rewrite nested operands inside table indexes, soundfile indexes, arrays,
  calls, selects, casts, and arithmetic nodes;
- return an explicit failure when a required rewrite cannot be represented;
- preserve existing interned nodes when no child changed.

Pass criterion: if a dependency is accepted because it is bufferable, every tail
use of that dependency is rewritten. An accepted split must pass FIR verification.

### H4 — Tighten `partition_recursive_body`

Make the partition algorithm reject ambiguous cases by default:

- no tail statement may read or write `Struct` state after rewriting;
- no serial statement may depend on a tail-produced value;
- nested control flow remains fallback-only until region-aware vector splitting
  is designed;
- statement kinds not explicitly supported by the split remain fallback-only;
- diagnostics should make fallback reasons inspectable in tests or debug dumps.

Pass criterion: partition unit tests cover APF-like table state, soundfile-index
dependencies, fully recursive bodies, unsupported statements, and successful
pure-tail buffering.

### H5 — Add impulse and backend gates

Use the vector-mode impulse targets as regression gates:

- start with `interp-vec0` and `interp-vec1` because they exercise FIR
  verification and runtime scope issues quickly;
- then run `cpp-vec0`, `cpp-vec1`, `c-vec0`, `c-vec1`, `cranelift-vec0`,
  `cranelift-vec1`, `wasm-vec0`, `wasm-vec1`;
- keep AssemblyScript in the same matrix, subject to its existing dependency
  availability and known-failure policy.

Pass criterion: any remaining failing DSP is either fixed, classified as an
unrelated backend/runtime bug, or added to a narrow known-failure list with a
follow-up plan.

### H6 — Decide when to revisit signal-level vectorization

After H1-H5 are green, evaluate whether FIR-level splitting is sufficient:

- correctness: no known wrong-code vector split;
- coverage: how often recursive slices fall back to serial;
- performance: how often the split pays versus the buffering cost;
- parity: differences versus Faust C++ `-vec -lv 0/1` loop structure.

Only if coverage/performance is clearly blocked should faust-rs start a dedicated
signal-level vectorizer port. That should be a separate plan because it changes
the signal-to-FIR lowering contract.

## 5. Validation matrix

Required before landing the hardening implementation:

- `cargo fmt --all`
- focused unit tests in `crates/transform/src/signal_fir/loop_graph.rs`
- focused compiler oracle tests in `crates/compiler/tests/vector_mode.rs`
- `cargo test -p transform loop_graph`
- `cargo test -p compiler vector_mode`
- `make -C tests/impulse-tests interp-vec0`
- `make -C tests/impulse-tests interp-vec1`

Recommended before declaring the `-vec` matrix healthy:

- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --all-targets`
- `make -C tests/impulse-tests all-vec0`
- `make -C tests/impulse-tests all-vec1`

## 6. Non-goals for this plan

- No new signal-level vectorizer in this step.
- No backend-specific workaround for APF or soundfile cases.
- No performance heuristic changes until correctness is locked.
- No broad FIR rewrite-engine migration unless H1 proves the local helper should
  immediately become shared infrastructure.

## 7. One-line summary

Make the current FIR-level vector split conservative, total, and symmetric:
collect every nested read, rewrite every accepted cross-boundary use, and fall
back to a serial loop whenever the proof is incomplete.
