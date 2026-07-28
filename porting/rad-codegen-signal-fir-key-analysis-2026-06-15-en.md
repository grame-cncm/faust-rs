# RAD `guitar_preamp_rad.dsp` Codegen Slowdown Analysis

**Date**: 2026-06-15  
**Status**: analysis documented, implementation deferred  
**Observed case**:
`/Users/letz/Developpements/Recherche/Emeraude/Thèse CIFRE/guitar_preamp_rad.dsp`
using:

```faust
process = rad(preamp, seeds);
```

with the same 33 explicit slider seeds used in the FAD preamp experiment.

## Summary

`guitar_preamp_rad.dsp` parses, evaluates, and propagates quickly enough. The
observed timeout is not caused by RAD propagation itself. It happens after
signal validation, inside the signal-to-FIR lowering path.

Representative timing:

```text
end parser (duration : ~0.38s)
end evaluation (duration : ~5.9s)
end box-flatten (duration : ~0.0006s)
end arity (duration : ~0.001s)
end propagation (duration : ~0.06s)
end signal-type-validation (duration : ~0.004s)
```

After that point, all of the following hit the compiler timeout:

```sh
faust-rs --compilation-time guitar_preamp_rad.dsp -o /tmp/out
faust-rs --compilation-time --dump-fir guitar_preamp_rad.dsp -o /tmp/out.fir
faust-rs --compilation-time --dump-cpp guitar_preamp_rad.dsp -o /tmp/out.cpp
faust-rs --compilation-time --no-fir-verify --dump-fir guitar_preamp_rad.dsp -o /tmp/out.fir
faust-rs --compilation-time --no-fir-verify --dump-cpp guitar_preamp_rad.dsp -o /tmp/out.cpp
```

Disabling FIR verification does not help, so `fir-verify` is not the bottleneck.
The backend C++ emitter is also not reached in the failing run. The slow phase
is `signal-fir`.

## Confirmed Blocking Point

Temporary trace instrumentation in
`crates/transform/src/signal_fir/module/build.rs` showed this sequence:

```text
trace build_module: start
trace build_module: before analyze_signal_sharing
trace build_module: after analyze_signal_sharing 0.002s
trace build_module: before prepare_delay_lines
trace build_module: after prepare_delay_lines 0.004s
trace build_module: before classify_reverse_time_outputs
trace build_module: after classify_reverse_time_outputs 0.004s reverse=33
trace build_module: has_forward_outputs=true has_reverse_outputs=true
trace build_module: before forward_output_by_sig_key
```

The process then stalls until interrupted or until the timeout fires.

The temporary instrumentation was removed after the measurement.

## Root Cause

The stall is caused by this block in `build_module`:

```rust
if has_reverse_outputs {
    // Readable structural fallback keys are only needed when the RAD
    // reverse-time loop must reconnect a delayed value to a forward output.
    lower.forward_output_by_sig_key = signals
        .iter()
        .enumerate()
        .filter_map(|(index, &sig)| {
            (!reverse_time_outputs[index]).then_some((dump_sig_readable(arena, sig), index))
        })
        .collect();
}
```

For this RAD program:

- total public outputs: 34;
- forward outputs: 1 primal;
- reverse-time outputs: 33 gradients;
- `has_forward_outputs == true`;
- `has_reverse_outputs == true`.

Because at least one reverse-time output exists, the lowerer eagerly builds a
string-key fallback map for every forward output. For the one primal output,
this calls:

```rust
dump_sig_readable(arena, primal_sig)
```

This is the same structural rendering problem documented in
`porting/dump-sig-fad-rendering-analysis-2026-06-15-en.md`: the signal graph is
a hash-consed DAG, but `dump_sig_readable` expands it as a tree. On the preamp
graph, the fully expanded text is effectively enormous.

Therefore code generation appears slow, but the immediate issue is not C++
string emission. It is a readable signal dump used internally as a lookup key
during signal-to-FIR lowering.

## Why This Key Exists

`SignalToFirLower` has two maps:

```rust
forward_output_by_sig: HashMap<SigId, usize>,
forward_output_by_sig_key: HashMap<String, usize>,
```

The first map is cheap and keyed by the prepared `SigId`. The second map is a
fallback described as:

```rust
/// Same map as [`Self::forward_output_by_sig`], keyed by the prepared
/// readable signal shape to survive equivalent but non-identical `SigId`s.
```

The fallback is used in `lower_forward_output_delay1_for_reverse_loop`:

```rust
let output_index = self.forward_output_by_sig.get(&value).copied().or_else(|| {
    self.forward_output_by_sig_key
        .get(&dump_sig_readable(self.arena, value))
        .copied()
});
```

The semantic goal is valid: during a reverse-time RAD loop, a contribution such
as `adjoint[n] * y[n-1]` must replay the already-computed forward primal output
instead of advancing a recursion carrier in reverse-time order.

The implementation problem is the use of full readable dumps as structural
keys. For large RAD/FAD-expanded graphs, this destroys DAG sharing and can
generate astronomical intermediate strings.

## Relationship to the FAD `--dump-sig` Issue

The FAD issue was user-visible:

```sh
faust-rs --dump-sig guitar_preamp_fad.dsp
```

It stalled because the CLI explicitly rendered all signals with
`dump_sig_readable`.

The RAD issue is internal:

```sh
faust-rs guitar_preamp_rad.dsp
```

It stalls because the FIR lowerer uses `dump_sig_readable` as a structural key
before actual lowering work starts.

Both have the same technical cause:

- compact internal DAG;
- tree-shaped string rendering;
- no sharing-preserving representation;
- no output/size guard.

## Possible Solutions

### Option A - Remove the String Fallback if It Is Obsolete

First determine whether `forward_output_by_sig_key` is still needed.

The prepared signal forest is built in one `TreeArena`, and `TreeArena` is
hash-consed. If the relevant forward primal and the delayed reverse-loop value
are structurally identical after preparation, the direct `SigId` map should be
sufficient.

Work:

1. add focused tests for the reverse-time replay cases that originally required
   the fallback;
2. temporarily remove or disable `forward_output_by_sig_key`;
3. verify existing RAD/BRA tests and representative recursive coefficient
   gradient cases.

Advantages:

- simplest and fastest runtime behavior;
- no structural key maintenance;
- avoids collision/format concerns.

Risks:

- if the fallback covers a real non-identical-but-equivalent case, removing it
  can regress reverse-time RAD semantics.

Recommendation: do this audit first. If the fallback is no longer necessary,
remove it.

### Option B - Replace Readable Dumps With Compact Structural Fingerprints

If a fallback remains necessary, replace `HashMap<String, usize>` with a compact
DAG-aware key.

Possible shape:

```rust
forward_output_by_sig_key: HashMap<SigFingerprint, usize>
```

where `SigFingerprint` is computed with memoization:

```rust
fingerprint(node) = hash(kind(node), fingerprint(child0), ...)
```

Requirements:

- compute each reachable node fingerprint once;
- preserve enough structure to distinguish signal kinds, tag names, constants,
  binary operators, control ids, and child order;
- avoid building large strings;
- optionally keep a collision path:
  `HashMap<SigFingerprint, Vec<(SigId, usize)>>` plus a DAG-aware structural
  equality check when fingerprints collide.

Advantages:

- keeps the current fallback semantics;
- runtime and memory proportional to unique node count;
- reusable for other structural lookup needs.

Risks:

- must be deterministic;
- must handle all signal node kinds used after preparation;
- collision handling must be explicit if correctness depends on equality.

Recommendation: preferred if Option A proves unsafe.

### Option C - Use a Prepared-Origin Map Instead of Structural Equivalence

Teach `prepare_signals_for_fir_verified` to expose a mapping from source signal
ids to prepared signal ids, or from semantically important source nodes to their
prepared equivalents.

The reverse-time replay logic could then match the reverse-loop value to the
public primal through provenance rather than through stringified structure.

Advantages:

- avoids structural hashing where provenance is enough;
- explicit relationship between source and prepared trees;
- likely more robust than comparing readable strings.

Risks:

- changes the preparation API;
- must be carefully documented because preparation may clone, promote, and
  simplify signals;
- provenance through simplification can be non-trivial.

Recommendation: good long-term design if more passes need source/prepared
correspondence.

### Option D - Lazy Fallback Key Construction

Do not eagerly build `forward_output_by_sig_key` for all forward outputs. Compute
fallback keys only when direct `SigId` lookup fails in
`lower_forward_output_delay1_for_reverse_loop`.

Advantages:

- avoids work when direct lookup succeeds;
- small localized change.

Limits:

- if direct lookup fails on a huge value, it still calls `dump_sig_readable` and
  can stall;
- does not solve the underlying string-key problem.

Recommendation: acceptable as a short-term mitigation only if combined with
Option B or a size guard.

### Option E - Add a Guard Against Huge Internal Dumps

Forbid internal calls to `dump_sig_readable` on large graphs unless explicitly
requested for diagnostics.

Possible policy:

- add a bounded dump helper;
- return `<too-large sig=...>` after a node/byte/depth limit;
- never use unbounded readable dumps as semantic keys.

Advantages:

- prevents future hidden stalls;
- improves failure behavior.

Limits:

- a truncated string is not a valid semantic key;
- this is a guardrail, not a correctness-preserving replacement.

Recommendation: add as a defensive measure, but do not use truncated text for
matching semantics.

## Proposed Correction Plan

### Phase 1 - Reproduce and Guard

Deliverables:

- add a regression fixture based on a reduced `rad(preamp, seeds)` shape or the
  full `guitar_preamp_rad.dsp` if it can be made repository-portable;
- add a targeted unit/integration test that exercises
  `lower_forward_output_delay1_for_reverse_loop` without requiring a huge DSP;
- add timing/assertion coverage proving `signal-fir` does not call unbounded
  `dump_sig_readable` on the public primal path.

Pass criteria:

- the reproducer reaches `signal-fir`;
- the test fails or times out on the current implementation;
- the test is small enough for CI.

### Phase 2 - Audit Fallback Necessity

Deliverables:

- identify the historical case that required `forward_output_by_sig_key`;
- run RAD/BRA tests with the string fallback disabled;
- document whether direct `SigId` matching is sufficient after current signal
  preparation.

Pass criteria:

- either prove the fallback can be removed safely;
- or document at least one concrete case where non-identical prepared `SigId`s
  must still match semantically.

### Phase 3A - If Fallback Is Unnecessary: Remove It

Deliverables:

- remove `forward_output_by_sig_key`;
- remove internal `dump_sig_readable` calls from reverse-time replay lookup;
- keep or add tests covering reverse-time primal replay.

Pass criteria:

- `guitar_preamp_rad.dsp` progresses past `signal-fir` key setup;
- existing RAD/BRA tests pass;
- no unbounded readable dump remains on the lowering hot path.

### Phase 3B - If Fallback Is Necessary: Replace It

Deliverables:

- implement a memoized structural fingerprint for prepared signals;
- replace `HashMap<String, usize>` with `HashMap<SigFingerprint, ...>`;
- add collision-safe structural equality if needed;
- use the same fingerprint logic for eager or lazy lookup.

Pass criteria:

- key construction is proportional to unique reachable nodes;
- no readable dump is used as a semantic key;
- `guitar_preamp_rad.dsp` gets through `signal-fir` key setup quickly;
- representative recursive RAD coefficient-gradient tests remain correct.

### Phase 4 - Defensive Cleanup

Deliverables:

- audit all non-test calls to `dump_sig_readable`;
- classify each use as diagnostic-only or semantic;
- replace semantic uses with stable ids, fingerprints, or provenance maps;
- optionally add bounded diagnostic dump helpers.

Pass criteria:

- no unbounded readable structural dump is used in compiler hot paths;
- diagnostic dumps are either explicit CLI/debug behavior or bounded.

## Recommended Path

Start with **Phase 2** before implementing a replacement. The comment says the
fallback is needed for "equivalent but non-identical `SigId`s", but this should
be revalidated against the current prepared-signal pipeline. If the direct
`SigId` map is now sufficient, removal is the cleanest fix.

If the fallback is still required, implement **Option B**: a memoized structural
fingerprint. It preserves the intended semantics without converting large DAGs
to expanded strings.

Do not fix this by memoizing `dump_sig_readable` strings. That may reduce some
CPU recomputation, but it does not address the fundamental issue: the semantic
key should not be a fully expanded human-readable dump.

## Notes

This issue is separate from, but related to:

- `porting/dump-sig-fad-rendering-analysis-2026-06-15-en.md`

Both cases should be considered when designing any shared signal-DAG dump or
fingerprint utility.
