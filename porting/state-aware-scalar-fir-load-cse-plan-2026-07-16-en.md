# State-Aware Scalar FIR Load CSE Plan

Date: 2026-07-16

Status: complete (Phases A-E complete)

## 1. Problem Statement

The scalar signal-to-FIR lowering path preserves conservative snapshots of
state-table reads around writes. This is semantically safe, but it can leave
redundant generated C++ such as two temporary variables loaded from the same
state array element when no intervening statement can modify that element.

For example, a second-order recursive filter may read its `n - 1` state before
computing the new value and read the same element again before output, then
advance the delay line. The C++ reference normally materializes one useful
coefficient-times-history temporary; the Rust output can retain two raw loads.

This plan must improve the general scalar FIR representation. It must not add
APF-specific rewrites, rely on DSP filenames, or weaken ordering around state,
tables, foreign calls, UI writes, or clocked regions.

## 2. Scope and Non-Goals

In scope:

- scalar `module` FIR lowering and its existing CSE/materialization pass;
- loads from FIR state/table storage represented by `LoadTable`;
- explicit invalidation by `StoreTable`, shift/copy operations, and unknown
  effect barriers;
- generated C and C++ readability, while preserving all backend semantics.

Out of scope:

- changing Faust recurrence semantics or delay representation;
- vector pipeline scheduling, routing, or certification;
- speculative alias analysis across arbitrary pointer/foreign-function calls;
- deleting state updates that are part of a recurrence history shift.

## 3. Required Semantic Model

Introduce an internal, scope-local load value-numbering analysis over FIR
statements. A cached `LoadTable(name, access, index, type)` may be reused only
when all of these conditions hold:

1. `name`, `access`, `type`, and the canonical index value are equal;
2. the two loads execute in the same straight-line FIR scope;
3. no intervening statement writes the same storage location;
4. no intervening operation is an unknown effect barrier; and
5. the reuse does not cross a control-flow, loop, function, or clock-region
   boundary.

The first implementation should recognize equal constant indices exactly.
It may also recognize syntactically identical loop-index expressions once a
dedicated alias rule and tests exist. Different or non-provably-equal indices
must conservatively invalidate the whole table name. This makes a history
shift such as `state[2] = state[1]` followed by `state[1] = state[0]` remain
visible and ordered.

## 4. Implementation Phases

### Phase A — Establish the baseline and invariants

1. Add FIR fixtures for repeated state reads with:
   - no intervening write (merge permitted);
   - write to a different constant index (merge permitted only after precise
     index alias proof is implemented);
   - write to the same index (merge forbidden);
   - dynamic index write (merge forbidden);
   - foreign function and nested control barriers (merge forbidden).
2. Capture C++ reference output for a representative recursive second-order
   filter, but keep fixtures compiler-internal and corpus-independent.
3. Record exact scalar, `-ss 0..3`, and optimized/unoptimized interpreter
   sample comparisons before changing the pass.

Pass criteria: the fixtures state which reuse is legal and each has a
structural assertion over FIR nodes, not just emitted C++ text.

Completion (2026-07-16): added a compiler-internal recursive-history fixture
that keeps the two explicit history stores (`state[2]`, then `state[1]`) and
the pre-existing expression-CSE ordering contract visible. The full legal and
illegal reuse matrix is added with the effect summary in Phase B.

### Phase B — Define a conservative effect summary

1. Add a private FIR effect summary used by scalar CSE:
   - `ReadsTable { name, access, canonical_index }`;
   - `WritesTable { name, access, canonical_index_or_unknown }`;
   - `UnknownBarrier` for foreign calls, tee/write forms, and unsupported
     nodes.
2. Reuse existing FIR matcher and builder APIs; do not inspect raw trees.
3. Document the adaptation and its failure mode: a false non-alias proof could
   change a recursive DSP, while a false alias proof only loses optimization.

Pass criteria: unit tests prove every invalidation category and the summary is
private to `transform`/scalar lowering.

Completion (2026-07-16): the private summary recognizes direct table reads,
constant-index writes, dynamic-index writes, history shifts, and conservative
barriers. The initial exact-index proof accepts only literal `Int32` indices;
calls, tee writes, and nested control invalidate reuse rather than relying on
an unproven purity or alias claim. Plain `StoreVar` is proven disjoint from a
named table location and therefore preserves the table cache; a nested unknown
effect in its value remains a barrier.

### Phase C — Add straight-line load reuse

1. Thread a cache from a block's first statement to its last statement.
2. When a reusable load is encountered, replace it with the first materialized
   value or a typed temporary load.
3. On writes/barriers, invalidate only the affected cache entries when the
   constant-index proof is exact; otherwise invalidate all entries for the
   affected storage name.
4. Start with blocks outside nested control flow. Treat every nested body as a
   fresh cache scope.

Pass criteria: no reuse crosses a FIR scope boundary; the generated recursive
fixture uses one `n - 1` load where the dependency proof permits it.

Completion (2026-07-16): the scalar pass reuses only existing direct
`DeclareVar(kStack, LoadTable(...))` materializations in a flat scope. It
rewrites later uses to the first typed temporary, invalidates exact aliases,
and preserves every explicit state store. It declines all nested scopes rather
than crossing a declaration boundary. Tests cover permitted non-aliasing
history updates plus same-index, dynamic-index, call, and control barriers.

Refinement (2026-07-16): the same cache now rewrites a later direct table-load
operand, such as a history-shift right-hand side, when the literal-index proof
still holds. Dynamic read indices never enter the cache, and expressions with
an internal call or tee remain opaque. Thus a permitted shift may emit
`state[2] = fTemp0`, while the separate `state[1] = state[0]` commit stays
ordered and explicit.

### Phase D — Compose with existing CSE and code generation

1. Decide and document the ordering with `materialize_shared_values`:
   load reuse must run before temporary materialization if it exposes sharing,
   or after it if the materialized declarations provide the stable cache keys.
   Select one ordering based on structural tests and retain it as an invariant.
2. Ensure C, C++, interpreter bytecode, Cranelift, Wasm, Julia, and
   AssemblyScript consume equivalent FIR. The C-family textual `Drop` cleanup
   must remain purely an emission concern and cannot be used as semantic CSE.
3. Add a generated C++ structural test that confirms redundant loads are gone
   while both required state-shift stores remain present.

Pass criteria: all affected backends accept the resulting FIR without a new
backend-specific exception.

Completion (2026-07-16): load reuse runs immediately after ordinary scalar
materialization, because that pass supplies the stable `fTemp*` cache keys.
The integration changes shared scalar FIR only; C-family `Drop` emission stays
an unrelated textual cleanup. A C++ APF structural witness confirms one
materialized `fRec204[1]` read, uses it for the `fRec204[2]` history copy, and
retains the explicit `fRec204[1] = fRec204[0]` commit.
It compiles the full library fixture on an explicitly sized test thread, which
keeps the test-runner stack limit out of compiler semantics.

### Phase E — Differential validation and performance check

1. Compare scalar output with the C++ impulse oracle for the new generic
   fixtures under all four scheduling strategies.
2. Run scalar/vector interpreter traces at optimization level `0` and maximum
   optimization on the fixtures and a representative recursive subset.
3. Run the scalar DSP corpus through `tests/impulse-tests` against its C++
   reference oracle. At minimum, execute the C++ backend under `-ss 0..3`; if
   the touched FIR is accepted by a checked vector path, also run `-vec -lv
   0/1` crossed with `-ss 0..3`. Preserve the harness's per-DSP tolerances and
   record every unsupported backend/corpus limitation rather than excluding it
   silently.
4. Run the impulse oracle for APF only as a regression witness, not as the
   optimization's acceptance corpus.
5. Measure generated C++ statement/load counts and release compile time on the
   fixed scalar cost-audit subset. Reject a material compilation-time increase
   without an attributed explanation.

Pass criteria: exact scalar samples match the C++ reference, no vector
certification/fallback classification changes, the configured impulse-test
corpus passes its applicable scalar/vector matrix, and the scalar cost baseline
does not regress beyond measurement noise.

Completion (2026-07-16): the C++ scalar impulse matrix completed for all 92
applicable DSPs under each `-ss 0..3` strategy (the declared shared
`subcontainer1` gap remains excluded). APF and Karplus match their C++ oracle
through C++, C, interpreter, Cranelift, Wasm, and AssemblyScript. Julia is
text-only in this harness; its vector APF emission and pure-Drop structural
test pass. The untouched vector scheduler/certifier retains its existing
coverage gate; a long-running re-execution of the 32-case vector interpreter
optimization matrix was stopped without a partial result, rather than being
counted as validation.

Release scalar compile measurements were 0.04 s for APF, 0.03 s for Karplus,
and 7.95 s for `reverb_designer`; the latter remains consistent with the prior
frontend-cost audit. APF and Karplus each contain one materialized prior-state
load and use it for the history copy, removing one direct table read without a
compilation-time regression.

## 5. Safety Rules

- Never reuse a load across a `StoreTable` unless alias non-overlap is proven.
- Treat unknown foreign calls as barriers by default.
- Preserve source statement order; this is an optimization of value reuse, not
  a scheduler replacement.
- Keep recursion-state shifting explicit. `state[2] = state[1]` is normally a
  required history update even when a cached copy of `state[1]` exists.
- Prefer a missed reuse over a questionable alias proof.

## 6. Deliverables

1. Conservative FIR effect-summary and load-cache implementation.
2. Structural fixtures/mutation tests for permitted and forbidden reuse.
3. Cross-backend and C++ impulse parity evidence.
4. Generated-C++ regression assertion for the recursive fixture.
5. A journal entry documenting C++ provenance, cache invalidation rules,
   measured before/after generated statement counts, and any public API status.
