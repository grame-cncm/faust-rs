# `--dump-sig` slowdown analysis on multi-parameter FAD

**Date**: 2026-06-15  
**Status**: analysis documented, implementation deferred  
**Observed case**: `guitar_preamp_fad.dsp`, using `fad(preamp, seeds)` with 33
explicit slider parameters.

## Summary

Normal compilation of `guitar_preamp_fad.dsp` is not the problem. The slowdown
appears with:

```sh
faust-rs --dump-sig guitar_preamp_fad.dsp
```

The diagnostics show that compilation up to propagated signals completes in a
few seconds, then the process spends its time rendering the signal text.

Representative measurement with `--compilation-time --dump-sig`:

```text
end parser (duration : ~0.37s)
end evaluation (duration : ~5.7s)
end box-flatten (duration : ~0.0005s)
end arity (duration : ~0.001s)
end propagation (duration : ~1.2s)
end signal-type-validation (duration : ~0.08s)
```

After `signal-type-validation`, no output file has been written yet when `-o`
is used, because the CLI first builds one large `rendered` string.

## Problem Location

The `--dump-sig` CLI path is in `crates/compiler/src/cli/runner.rs`:

```rust
let mut rendered = format!(
    "Signals OK: inputs={} outputs={}",
    out.process_arity.inputs, out.process_arity.outputs
);
for (index, sig) in out.signals.iter().enumerate() {
    rendered.push('\n');
    rendered.push_str(&format!(
        "[{index}] {}",
        dump_sig_readable(&out.parse.state.arena, *sig)
    ));
}
rendered.push('\n');
emit_output(&rendered, cli.output.as_ref());
```

`dump_sig_readable` lives in `crates/signals/src/lib.rs`. It calls
`dump_node_iter`, an iterative traversal that prints each signal as a complete
structural expression.

This traversal has no `visited` table and no definition table for shared
subgraphs. The hash-consed `TreeArena` therefore represents a compact DAG, but
`dump_sig_readable` expands it as a tree.

## Measurements on `guitar_preamp_fad.dsp`

A temporary local observation tool counted signal graph properties without
changing the compiler:

```text
compile_to_signals_ms ~= 7300-8000
outputs = 34
unique_reachable_nodes = 15465
has_cycle = false
expanded_nodes_per_output > 1_000_000_000_000
```

Important points:

- there is no cycle in the outputs;
- the signal graph is compact: roughly 15k unique reachable nodes;
- the explosion comes only from unshared rendering;
- the 34 outputs are the primal plus 33 FAD tangent lanes;
- every output reprints massively shared subexpressions.

The problem is therefore not `fad` itself. A small recursive FAD example such as
`tests/corpus/fad_recursive_delay.dsp` remains tiny when dumped:

```text
outputs = 2
unique_reachable_nodes = 32
expanded_nodes_per_output = 67
```

The pathological case is the combination of:

- realistic DSP with many filters and shared subexpressions;
- multi-seed FAD, hence many tangent outputs;
- structural dump that loses `TreeArena` sharing.

## Root Cause

`--dump-sig` uses a tree-shaped text format while the internal signal IR is a
hash-consed DAG.

For a node shared by many parents, the current dump reprints the node's full
subtree at every occurrence. In FAD, tangent lanes often share the same primals,
coefficients, filters, and derivative subexpressions. The expanded text volume
becomes astronomical even when the real IR remains reasonable.

This also explains why redirecting to a file is not enough: the file is written
only after the full `rendered` string has been built.

## Possible Solutions

### Option A - DAG Dump With Shared Definitions

Add a dump format that preserves sharing:

```text
%0 = SIGINPUT(int(0))
%1 = SIGBINOP(op=mul (*), %0, ...)
%2 = SIGBINOP(op=add (+), %1, ...)
outputs = [%2, %17, %31, ...]
```

Principle:

1. traverse the outputs and count references for each `SigId`;
2. assign stable identifiers to reused nodes, or to all nodes;
3. emit each node once;
4. emit outputs as references.

Advantages:

- complexity is proportional to the number of unique nodes;
- preserves full structural information;
- suitable for large FAD/RAD programs;
- also useful for golden tests if ordering is deterministic.

Drawbacks:

- changes the output format if applied directly to `--dump-sig`;
- requires a clear compatibility policy for existing golden/tests.

Recommendation: first introduce a new mode, for example `--dump-sig-dag`, then
decide later whether `--dump-sig` should switch to it.

### Option B - Local Memoization in `dump_sig_readable`

Keep the current format, but memoize rendered subgraphs:

```rust
HashMap<SigId, String>
```

Advantages:

- easy to implement;
- reduces CPU recomputation during rendering.

Limits:

- does not reduce final output size if the format still reinserts subgraph text
  at every occurrence;
- can greatly increase memory use by storing very large strings;
- does not solve cases where the fully expanded textual output is intrinsically
  huge.

Conclusion: useful only as a micro-optimization, not as the main solution.

### Option C - Stream Directly to `Write`

Change `dump_sig_readable` to write into a caller-provided `impl Write` or
`fmt::Write`, instead of building a large `String` and writing it afterward.

Advantages:

- avoids the memory peak from `rendered`;
- makes output progressive;
- makes `-o` truly streaming.

Limits:

- does not reduce the combinatorial expansion;
- total runtime remains prohibitive if the format is still expanded as a tree.

Conclusion: improves ergonomics and memory behavior, but is insufficient alone.

### Option D - Dump Limits and Controlled Truncation

Add safety options:

```sh
--dump-sig-max-nodes N
--dump-sig-max-bytes N
--dump-sig-max-depth N
```

Example rendering:

```text
SIGBINOP(op=add (+), <truncated sig=1234>, ...)
```

Advantages:

- avoids commands that appear to hang;
- gives an actionable diagnostic for very large graphs;
- simple to integrate into the existing `--dump-sig` path.

Limits:

- output is incomplete;
- less suitable for exhaustive structural tests.

Recommendation: useful as a guardrail even if a DAG dump is added.

### Option E - Signal Statistics Summary

Add a synthetic mode:

```sh
--dump-sig-summary
```

Possible output:

```text
Signals OK: inputs=1 outputs=34
unique_reachable_nodes=15465
root_tags:
  SIGBINOP: 34
expanded_tree_nodes_capped=>1e12
```

Advantages:

- fast diagnostic;
- suitable for large FAD/RAD programs;
- avoids confusing compilation time with rendering time.

Limits:

- does not replace a complete structural dump when formulas need inspection.

### Option F - Selective Dump by Output or Subgraph

Add options:

```sh
--dump-sig-output 0
--dump-sig-output 17
--dump-sig-root <id>
```

Advantages:

- allows focusing on a specific tangent lane;
- reduces volume when only one output matters.

Limits:

- does not solve expansion of a single output if that output already expands a
  large shared DAG.

## Proposed Plan

### Phase 1 - Diagnostics and Guardrails

- Add `--dump-sig-summary`.
- Add a default or optional limit for `--dump-sig`.
- Clearly report when the dump is truncated.

Pass criteria:

- `guitar_preamp_fad.dsp` produces a summary in under 10 seconds;
- `--dump-sig` can no longer run silently for minutes without producing a
  diagnostic.

### Phase 2 - DAG Dump

- Add a shared `dump_sig_dag_readable` dumper.
- Introduce a CLI option `--dump-sig-dag`.
- Emit stable, deterministic identifiers independent of memory addresses.

Pass criteria:

- output size is proportional to unique node count;
- `guitar_preamp_fad.dsp` can be dumped in reasonable time;
- unit tests cover:
  - shared subgraphs;
  - multi-seed FAD;
  - deterministic ordering;
  - no regression in the current dump.

### Phase 3 - Compatibility Policy

- Decide whether `--dump-sig` remains the historical expanded dump;
- or whether it becomes an alias for the DAG dump, with an explicit option for
  the old behavior, for example `--dump-sig-expanded`.

## Recommendation

The robust fix is **Option A: DAG dump with shared definitions**.

Options C, D, and E are complementary:

- C improves memory behavior and progress visibility;
- D avoids perceived hangs;
- E gives a fast diagnostic.

Option B alone is not sufficient, because it may speed up internal construction
but does not change the final expanded text size.

## Compatibility Notes

`dump_sig_readable` is used by differential tests and structural assertions. Its
format should not be changed abruptly without an audit.

Prudent approach:

1. keep `dump_sig_readable` unchanged;
2. add a new DAG dumper;
3. progressively migrate large or FAD/RAD-heavy tests to the DAG format;
4. document explicitly which format is intended for humans, golden tests, or
   exhaustive debugging.
