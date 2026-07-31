# Signal-to-FIR Generated-Code Optimization Roadmap

Date: 2026-07-31

Status: analysis and implementation roadmap; no optimization in this document
is considered active until its own legality, profitability, and validation
gates pass.

## 1. Purpose

This document takes a step back from individual generated-code differences and
asks a broader question:

> Which domain facts can the Signal-to-FIR compiler still preserve or expose so
> that every backend can generate better runtime code?

The recent BPF case is a useful example. Reusing a materialized old recursion
value:

```cpp
double fTemp0 = fRec0[1];
// ...
fRec0[2] = fTemp0;
```

is semantically just a proven redundant-load elimination. In generated C++ with
`FAUSTFLOAT=double`, however, it also prevents a reload that Clang may otherwise
retain because an output pointer could alias DSP state. The optimization is
therefore valuable at the common FIR level: it expresses a domain fact that a
general-purpose compiler cannot always reconstruct from the public C++ ABI.

The general conclusion is not "add more textual peepholes". The Signal-to-FIR
stage should:

1. preserve high-level DSP facts while they are still available;
2. prove transformation legality independently of profitability;
3. use a cost model before changing storage, scheduling, or loop shape;
4. keep default floating-point evaluation order stable; and
5. optimize shared FIR whenever possible, leaving only target-specific facts to
   backends.

## 2. Scope and non-goals

In scope:

- scalar and checked-vector Signal-to-FIR lowering;
- FIR-to-FIR canonicalization before backend emission;
- state, delay, table, I/O, and temporary-memory traffic;
- expression placement, materialization, scheduling, and loop shape;
- facts that help C, C++, Rust, Wasm, Cranelift, interpreter, Julia, and
  AssemblyScript backends;
- measurement and validation required to qualify runtime optimizations.

Out of scope:

- changing Faust DSP semantics to win a benchmark;
- assuming that host audio buffers never alias unless the public architecture
  contract proves it;
- default floating-point reassociation, reciprocal approximation, or FMA
  contraction when they can change samples;
- backend-specific source rewrites masquerading as FIR semantics;
- replacing downstream optimizing compilers with a machine-instruction
  optimizer in `transform`.

The default objective remains semantic parity with C++ Faust. An optimization
may intentionally produce code unlike the C++ reference when it is proven
equivalent and is validated by the differential oracle.

## 3. Current baseline

The current implementation already performs substantial domain-aware
optimization. Future work must build on it rather than duplicate it.

| Domain fact | Current mechanism | Important remaining limit |
| --- | --- | --- |
| Init, control, and sample variability | [`placement.rs`](../crates/transform/src/signal_fir/placement.rs) places values in lifecycle buckets | Placement and later materialization use no unified cost/lifetime model |
| Shared pure expressions | [`cse.rs`](../crates/transform/src/signal_fir/cse.rs) materializes shared `FirId` values | The threshold is mostly structural, not target- or pressure-aware |
| Repeated scalar state reads | `reuse_straight_line_scalar_loads` uses literal-index alias facts | Flat scopes only; calls, dynamic indices, and nested control stop the analysis |
| Delay storage | [`delay/`](../crates/transform/src/signal_fir/delay) selects shift, power-of-two circular, or exact wrapping strategies | Thresholds are options, not a measured target/backend cost decision |
| Scalar execution order | Hierarchical graph plus `-ss 0..3` scheduling | Strategies are legal schedules, not explicit register-pressure/memory-cost optimizers |
| Vector execution | Checked analysis, planning, routing, event/state certificates, fission, and lockstep vectorization | Every admissible split is still taken; longer-delay lockstep state lacks the planned SoA layout |
| Dead pure scaffolding | Pure `Drop` roots are removed after their proof role is consumed | There is no general common-FIR dead-code/dead-store pass |
| Helper calls | [`fir::inliner`](../crates/fir/src/inliner.rs) has conservative inlining machinery | It is not a general production profitability framework for generated DSP helpers |
| FIR/IIR forms | Recognition and carrier algebra exist | Production carrier reveal and filter-specialized lowering remain inactive; see the [activation plan](fir-iir-reveal-activation-plan-2026-07-20-en.md) |

Two existing plans are direct foundations:

- the [runtime placement and CSE plan](fir-cse-runtime-optimizations-plan-2026-04-03-en.md);
- the completed [state-aware scalar load-CSE plan](state-aware-scalar-fir-load-cse-plan-2026-07-16-en.md).

The [scheduling/vectorization review](scheduling-vectorization-implementation-review-2026-07-16-en.md)
already identifies two major profitability gaps: unconditional vector fission
and the absence of longer-delay SoA state.

## 4. The optimization boundary

### 4.1 What Signal-to-FIR knows that a backend may not

Signal-to-FIR has exact knowledge of:

- which values are init-, control-, or sample-rate;
- which state slot represents which recursion or delay history;
- the required order of state reads and commits;
- whether two table names are distinct compiler-owned objects;
- whether an index is literal, affine in the sample index, masked, or unknown;
- which calls are canonical math operations and which are foreign/unknown;
- loop-carried dependencies and independent signal subgraphs;
- maximum delays and selected storage geometry;
- the numerical policy under which a rewrite was authorized.

These facts should either drive common FIR transformations or survive as typed
FIR metadata. Once lowered to unrestricted pointers and calls in C/C++, some of
them are expensive or impossible to recover.

### 4.2 What should remain downstream

Backends and native compilers should retain responsibility for:

- machine instruction selection and scheduling;
- physical register allocation;
- exact SIMD width and instruction legality for the target CPU;
- target latency/throughput tables;
- ABI spelling of `restrict`, `noalias`, alignment, and calling-convention
  attributes;
- final scalar-versus-vector choice when it depends on target features.

The common FIR should expose facts, not imitate one downstream optimizer.

### 4.3 Three independent decisions

Every proposed optimization needs three separate answers:

1. **Legality:** does it preserve observable DSP state, I/O, UI/table effects,
   call order, and numerical semantics?
2. **Profitability:** does it reduce expected runtime cost after accounting for
   loads, stores, code size, transports, and register pressure?
3. **Policy:** is the transformation enabled under the selected numerical and
   ABI contract?

A proof of legality is not a profitability result. LLVM's loop-fusion
documentation makes the same limitation explicit: legal fusion without a cost
model can lose on cache footprint, register pressure, or downstream
vectorization.

## 5. Opportunity catalogue

### 5.1 Generated-code observability before new rewrites

The first missing optimization facility is measurement, not a rewrite.

Add deterministic FIR/source metrics per compilation:

- arithmetic, cast, call, load, and store counts by lifecycle bucket;
- state/table accesses classified by literal, affine, or unknown index;
- number and live-range span of materialized temporaries;
- estimated maximum simultaneously live scalar values per loop;
- state bytes, temporary bytes, vector transport bytes per block, and code size;
- loop count, trip-count shape, recurrence edges, vectorizable operations, and
  scalar remainder work;
- optimization decisions with stable reason codes: applied, illegal, or legal
  but unprofitable.

These metrics provide a stable explanation for benchmark changes. Source-line
counts alone are insufficient: replacing one expression with a temporary can
add a line while removing a machine load, as BPF demonstrated.

Initial instrumentation must be observation-only and included in the
compilation-cost gate. It should make no emitted-FIR decision.

### 5.2 One common FIR effect and memory-location model

The existing scalar load cache and vector pipeline each reconstruct part of the
effect model they need. A small shared vocabulary would unlock more
optimizations without sharing mutable checker state.

Proposed semantic facts:

```text
MemoryObject =
    DspState(field)
  | DelayLine(id)
  | RecursionState(group, lane)
  | ReadOnlyTable(id)
  | MutableTable(id)
  | Input(channel)
  | Output(channel)
  | UiZone(id)
  | Soundfile(id)
  | ForeignOrUnknown

IndexClass =
    Scalar
  | Literal(i)
  | Affine { induction, scale, offset }
  | MaskedAffine { induction, offset, mask }
  | Unknown

Effect = Read(location) | Write(location) | ReadWrite(location) | Barrier
```

The alias relation should be deliberately asymmetric:

- different compiler-owned objects are `NoAlias`;
- different literal slots in the same object are `NoAlias`;
- equal literal slots are `MustAlias`;
- simple affine/masked ranges may be disjoint only when a checked proof says so;
- unknown pointers, indices, and foreign calls remain `MayAlias`/barriers.

This is the FIR analogue of combining alias queries with Mod/Ref information.
LLVM's Alias Analysis documentation makes that combination central, and
MemorySSA shows how memory def/use versions can make clobber queries cheap
without repeated backward scans.

The first version should remain intraprocedural and scope-local. It does not
need a full pointer analysis: most Faust-owned state already has stronger
identity than a C pointer.

### 5.3 Memory value numbering beyond one flat block

With the common effect model, extend the proven BPF optimization into a
conservative memory-value framework:

- redundant-load elimination through nested straight-line blocks when
  dominance and scope are explicit;
- store-to-load forwarding for the exact same compiler-owned location;
- reuse across writes to proven-disjoint locations;
- loop-invariant load hoisting for read-only tables and block-rate state;
- partial redundancy elimination only after control-flow joins carry explicit
  memory versions;
- load sinking only when it shortens a live range without crossing a clobber.

A MemorySSA-like internal representation is a useful design reference, not a
requirement to reproduce LLVM. The smallest useful form may be per-object
version numbers attached to FIR statements:

```text
v0 = version(state[1])
t0 = load state[1] @ v0
store state[2]          // v0 for state[1] remains valid
use t0
store state[1]          // state[1] becomes v1
```

Control-flow joins, loops, unknown calls, volatile-like operations, atomics, and
dynamic writes must initially stop reuse. A false `NoAlias` result is a
correctness bug; a conservative barrier only misses an optimization.

### 5.4 Cost- and lifetime-aware materialization

The current CSE rule correctly avoids recomputing shared non-trivial
expressions, but materializing every structurally shared value is not always
best. A temporary can:

- replace expensive repeated work with a cheap register use;
- make old state explicit and defeat a downstream alias reload;
- extend a live range and cause spills;
- introduce stack traffic in non-optimizing backends;
- inhibit reassociation or vector packing;
- increase code size when the expression was cheap.

Introduce a backend-neutral first cost model based on:

```text
benefit =
    (uses - 1) * recompute_cost
  + proven_memory_loads_removed
  + alias-exposure_bonus
  - temporary_definition_cost
  - expected_reload_cost
  - live_range_pressure_penalty
```

The initial operation costs should be coarse and versioned:

- literals and variable loads: zero/trivial;
- integer/real add, comparison, cast: cheap;
- multiply/divide: increasing cost;
- table/state load: memory cost, adjusted for exact reuse;
- transcendental and foreign pure call: expensive;
- unknown/effectful call: never a CSE candidate.

Do not make the first version CPU-specific. Its purpose is to avoid obviously
bad materializations and retain obviously good ones. Native backends may later
override costs through a target profile, while interpreter/Wasm can keep their
own stable profile.

The BPF old-state temporary should remain profitable even if its arithmetic use
count is low: it removes a proven state read and communicates a stable value
across an output store.

### 5.5 Lifetime-aware scalar scheduling

All legal topological schedules are not equally good. Within the existing
effect and dependency constraints, statement priority can target:

- consume a temporary near its definition;
- complete one expression tree before opening another;
- delay expensive loads until just before use;
- commit state only after its last old-value use;
- minimize the peak live set;
- keep independent same-shape operations adjacent when SLP vectorization is
  likely.

This suggests a new costed scheduling strategy rather than silent changes to
the meaning of `-ss 0..3`. Candidate schedules can be scored by:

```text
score =
    peak_live_values * Wlive
  + weighted_live_range_sum * Wrange
  + state_loads * Wload
  + estimated_spills * Wspill
  - adjacent_isomorphic_ops * Wslp
```

The scheduler must continue to consume the same verified dependency/effect
graph. Only tie-breaking among legal ready nodes changes. Structural tests
should include adversarial DAGs where minimizing depth increases live ranges
and vice versa.

### 5.6 Scalar replacement of short-lived state

Compiler-owned state frequently remains in struct arrays during the entire
sample loop even when a short recurrence can live in scalar locals:

```cpp
double s1 = fRec[1];
double s2 = fRec[2];
for (...) {
    double next = ... s1 ... s2;
    // output
    s2 = s1;
    s1 = next;
}
fRec[1] = s1;
fRec[2] = s2;
```

This can remove per-sample state loads/stores and make recurrence dependencies
clear to register allocation. It generalizes the one-iteration old-state
temporary, but has a larger proof obligation:

- the state is private compiler-owned storage;
- no call, table alias, UI callback, or exposed method observes it mid-block;
- input/output buffers cannot alias it under the actual architecture layout;
- all exits commit the final values exactly once;
- `count == 0`, reverse-time loops, external `frame`, control separation, and
  exceptions/early returns preserve lifecycle semantics;
- recursive numerical order is unchanged.

Start only with fixed delay-one/small literal histories in scalar `compute`.
Never promote mutable tables or externally addressable storage. The final state
after each block must be compared, not only output samples.

### 5.7 Delay representation selected by cost, not only thresholds

The current shift/circular/exact-wrap abstraction is semantically clear.
Selection can become more profitable by considering:

- maximum delay and number of reads per sample;
- shift-store count versus masked-index arithmetic;
- whether the backend optimizes a constant small shift into registers;
- block size;
- code-size constraints;
- target preference for power-of-two masking versus branch/conditional wrap;
- whether several delays share one carrier and can reuse index arithmetic.

Possible improvements:

- hoist/reuse identical masked index expressions;
- keep very short histories in scalar locals;
- use circular buffers for medium histories;
- fold storage to the smallest proven live window;
- share cursor arithmetic only when update domains and clock contexts match;
- unroll small clear/copy loops only under a code-size budget.

Halide's storage-folding pass is a useful external analogue: buffer size is
reduced only when monotonic access and the required live window are known.
Faust has stronger delay semantics, so it can often prove the window directly.

### 5.8 Costed loop fission, fusion, and transport

Checked vector lowering currently prioritizes legality: an admissible fission
may introduce transport buffers even when a fused scalar loop is faster. The
next step is a profitability planner over already-certified alternatives.

For each candidate region, compare at least:

```text
fused cost =
    scalar operations
  + recurrence serialization
  + missed-vectorization penalty

fission cost =
    vector/scalar operation estimates
  + transport stores + transport loads
  + temporary buffer footprint
  + extra loop/control overhead
  + scalar remainder

fusion cost =
    reduced transport/loop overhead
  + increased live set
  + possible vectorization loss
```

Useful inputs:

- operation mix and loop trip count;
- known/default block size and vector size;
- number, type, and lifetime of transported values;
- vectorizer feasibility, not merely absence of a recurrence;
- code size and temporary storage;
- target/backend profile.

The output must record both legality and the selected cost estimate. Keep the
existing scalar fallback when the estimated speedup is below a conservative
margin. Benchmark evidence already shows that a legal split can be slower.

### 5.9 Layout transformations: AoS, SoA, and hot state

Layout is a domain decision because the compiler knows which dimensions are
lanes, delays, channels, and independent instances.

High-value candidates:

- longer-delay lockstep state as `state[delay][lane]` (SoA) so one delay tap is
  contiguous across SIMD lanes;
- contiguous transport buffers aligned to the backend's vector requirements;
- grouping hot scalar state separately from large cold tables/soundfile data;
- preserving per-clock-context ownership so sibling domains do not
  accidentally share storage;
- optional interleaving of independent channels only when it improves the
  selected backend's access pattern.

Layout changes are representation-level adaptations and require structural
certificates: element correspondence, lane/delay index mapping, bounds, update
order, and lifecycle clear/copy behavior. They also require multi-block final
state parity.

### 5.10 Filter-form recognition and specialized lowering

Generic expression lowering cannot always recover a profitable filter kernel.
Activating FIR/IIR reveal can expose:

- dense or sparse tap vectors;
- fixed versus control-rate coefficients;
- recurrence order and state-space shape;
- symmetry or repeated coefficients;
- opportunities for dot-product loops, unrolling, or vector reductions;
- common coefficient/state loads across outputs.

Specialized lowering should be driven by filter size and density:

- small fixed filters: straight-line code or a bounded unroll;
- larger dense filters: counted dot-product loop suitable for vectorization;
- sparse filters: explicit non-zero taps;
- control-rate coefficients: load/materialize once per block;
- multiple related outputs: share state reads and coefficient loads.

Default lowering must preserve expression order. Dot-product reassociation,
balanced reduction trees, reciprocal transforms, and FMA contraction belong to
an explicit relaxed-numerics mode unless exact equivalence is established for
the type and operation.

### 5.11 Late common-FIR canonicalization

A small post-lowering canonicalizer can simplify all backends:

- remove dead pure declarations after use recounting;
- eliminate identity casts and fold casts of literals;
- propagate single-use temporaries when this shortens, rather than extends,
  live ranges;
- canonicalize repeated literal/affine index arithmetic;
- remove stores overwritten before any possible read, but never the final
  externally persistent state commit;
- fold empty blocks and controls after proof scaffolding is consumed;
- strength-reduce exact integer address arithmetic, including power-of-two
  modulo where signed/range semantics prove equivalence.

This pass must operate on typed FIR and use the common effect model. Textual
cleanup in each emitter should be limited to syntax.

### 5.12 Backend facts: alias, alignment, purity, and access contracts

Some optimizations need facts that are best emitted as backend attributes:

- `noalias`/`restrict` only when the architecture ABI forbids overlap for the
  complete pointed-to object and call duration;
- `readonly`/pure attributes for canonical math or explicitly declared foreign
  functions;
- alignment for compiler-owned arrays and verified host buffers;
- loop vectorization/interleave hints only after a cost decision;
- target features and native vector widths only in target-aware backends.

`RESTRICT` is not a substitute for FIR reasoning:

- the public architecture may intentionally support in-place I/O;
- qualifying only the outer `FAUSTFLOAT**` table does not necessarily
  disambiguate the channel data reached through it;
- a compiler may still fail to exploit the annotation;
- interpreter and non-C backends receive no benefit.

The robust BPF fix is the explicit proven old-state value in common FIR.
Backend alias facts are an additional opportunity when the ABI genuinely
provides them.

### 5.13 Numerically relaxed optimizations as a separate lane

The default lane must not silently enable:

- floating-point reassociation;
- `a / b -> a * (1 / b)`;
- contraction into FMA;
- approximate transcendental functions;
- assumptions excluding NaNs, infinities, signed zero, or subnormals.

LLVM models these as distinct fast-math permissions because each changes what
rewrites are legal; notably, reassociation can materially change results. Faust
should likewise carry an explicit numerical policy into FIR and backend
emission.

A future relaxed lane may enable costed Horner forms, balanced reductions,
vector reductions, reciprocal reuse, and FMA. Its tests must compare against
the selected relaxed contract, not claim bit-exact parity.

## 6. Proposed optimization architecture

The common path should remain small and explicit:

```text
prepared signals
    |
    +-- optional high-level recognition (FIR/IIR, sparse form)
    |
Signal-to-FIR lowering
    |
FIR verification (semantic baseline)
    |
effect/location analysis --------------------+
    |                                        |
    +-- local memory value numbering         |
    +-- late canonicalization                | legality facts
    +-- costed materialization               |
    +-- lifetime-aware scalar scheduling     |
    |                                        |
verified optimized scalar FIR <--------------+
    |
    +-- or checked vector alternative planner
            +-- fusion/fission cost
            +-- layout/transport cost
            +-- certified vector assembly
    |
post-transform FIR verification
    |
backend facts + emission
```

Key boundaries:

- high-level recognition occurs before information is lost;
- general FIR cleanup occurs only after proof/certificate scaffolding has been
  consumed;
- every semantics-changing FIR rewrite is followed by verification;
- vector plan changes retain the existing producer/checker boundary;
- backend attributes never become the only correctness argument for common
  FIR.

Avoid one monolithic optimizer. Small passes should communicate through stable
summaries:

- `EffectSummary`;
- `MemoryLocation`;
- `ValueCost`;
- `LiveRangeSummary`;
- `LoopCost`;
- `NumericalPolicy`.

Producer and checker may share this vocabulary and pure predicates, but not
mutable analysis results whose agreement would cease to be independent.

## 7. Recommended implementation order

### R0 — Observation-only metrics

Deliver:

- deterministic FIR/source metrics and decision-report schema;
- fixed benchmark subset covering recursion, long delays, tables, filters,
  control-rate work, and vector transports;
- generated assembly/LLVM-vectorization evidence for selected native cases.

Pass criteria:

- byte-identical generated output;
- no meaningful compile-cost increase;
- metrics stable across repeated builds.

### R1 — Shared effect/location vocabulary

Deliver:

- compiler-owned memory-object identities;
- literal and simple affine index classes;
- Mod/Ref/barrier summaries;
- cross-checks showing current scalar and vector analyses agree on their common
  subset.

Pass criteria:

- observation-only at first;
- rejecting mutation tests for false non-alias cases;
- compile-budget gate stays green.

### R2 — Costed materialization and local memory optimization

Deliver:

- versioned coarse operation-cost table;
- temporary lifetime/pressure estimate;
- exact-location load reuse and store-to-load forwarding;
- late dead pure declaration/cast/index cleanup;
- stable applied/rejected reason codes.

Pass criteria:

- BPF retains one old-state load and its measured benefit;
- no representative benchmark regresses beyond the configured noise margin
  without an attributed target-profile exception;
- scalar oracle and final-state tests pass across `-ss 0..3`.

### R3 — Register promotion for short recurrence state

Deliver:

- scalar replacement for a narrowly certified delay-one/small-history subset;
- prologue load and epilogue commit verifier;
- explicit exclusions for externally observable/effectful storage.

Pass criteria:

- exact outputs and final state over zero, one, partial, and multiple blocks;
- optimized/unoptimized interpreter parity;
- C/C++/Rust/Wasm/Cranelift backend matrix on the qualified subset.

### R4 — Costed loop alternatives

Deliver:

- fused/fissioned candidate cost comparison;
- conservative benefit threshold and scalar fallback;
- benchmark-calibrated target profiles;
- retained certificate/checker validation for the chosen plan.

Pass criteria:

- known 0.92x-style fission losses are rejected;
- vector coverage does not silently shrink;
- scalar/vector numerical and final-state gates pass.

### R5 — Layout and filter-specialized kernels

Deliver:

- longer-delay lockstep SoA;
- FIR/IIR reveal activation and costed dense/sparse lowering;
- backend alignment/vector facts derived from verified layouts.

Pass criteria:

- structural layout certificate plus rejecting mutations;
- filter corpus parity at all supported precisions;
- demonstrated throughput or memory-footprint benefit on the intended class.

## 8. Validation discipline

Generated-code optimization needs four independent kinds of evidence.

### 8.1 Semantic evidence

- unit tests for each legality rule and each conservative rejection;
- FIR verifier before and after transformation;
- C++ differential impulse tests;
- optimized versus unoptimized interpreter execution;
- final DSP state, tables, and UI effects after multiple block sizes;
- zero-length and non-dividing block/vector sizes;
- scalar strategies `-ss 0..3`, applicable vector modes, and execution options
  such as external control/frame.

Numerical output alone is not sufficient for stateful code: an incorrect final
state can appear only in the next block.

### 8.2 Structural evidence

- exact access/load/store counts on minimal fixtures;
- no required state commit removed;
- no cache reuse across a mutation barrier;
- expected loop topology, transport count, and layout mapping;
- backend-independent FIR assertion first, emitted-source assertion second.

### 8.3 Performance evidence

Use the statistically auditable normal benchmark workflow:

```sh
make -C tests/impulse-tests bench \
  BENCH_OPTIONS="-double -run 5 -bs 512"
```

Also test targeted block sizes because the best loop/storage decision can change
between `count = 1`, common low-latency blocks, and `512`. Report geometric
mean, median, wins/losses, non-finite/unsupported cases, and named regressions;
do not accept an aggregate win that hides a severe class regression.

For native-code claims, inspect compiler optimization remarks and assembly/LLVM
IR on representative targets. A cleaner C++ source is evidence of intent, not
proof of fewer instructions.

### 8.4 Compilation-cost evidence

Every implementation phase in `transform`, `fir`, `codegen`, or `compiler`
must run:

```sh
cargo run --release -p xtask -- compile-budget-check
```

Optimization analysis must not recreate pairwise or backward-scan complexity
that becomes quadratic on large DSP graphs. Prefer cached def/use facts,
per-object indexing, and linear or near-linear passes.

## 9. Priorities at a glance

| Priority | Work | Expected leverage | Correctness risk | Main evidence |
| --- | --- | --- | --- | --- |
| P0 | Metrics and decision reports | Enables every later decision | Low | Output identity, compile budget |
| P1 | Shared effect/location model | Unlocks safe memory optimization | Medium | Mutation tests, analysis cross-check |
| P1 | Costed CSE/materialization | Broad scalar/all-backend improvement | Low–medium | BPF + corpus benchmark |
| P2 | Local memory value numbering | Fewer state/table loads | Medium | Alias/barrier matrix, multi-block oracle |
| P2 | Costed vector fission/fusion | Avoid known slow legal plans | Medium | `vec-bench`, certificates |
| P3 | Short-state register promotion | Large recursive-kernel potential | High | Lifecycle/final-state verifier |
| P3 | Longer-delay SoA | High lockstep SIMD leverage | High | Layout certificate, native SIMD evidence |
| P3 | FIR/IIR specialized lowering | High on recognized filters | High | Recognition checker, filter corpus |
| Separate policy | Relaxed FP transforms | Potentially high | Semantic by design | Explicit numerical contract |

The recommended first runtime change is costed materialization built on an
observation-only lifetime/effect summary. It is incremental, benefits every
backend, preserves the successful BPF state snapshot, and supplies
infrastructure needed by the riskier register-promotion and loop-planning work.

## 10. External references

The references below are design analogues, not specifications for Faust:

- Faust documentation, [Optimizing the Code](https://faustdoc.grame.fr/manual/optimizing/):
  variability tiers, scalar/vector code shapes, and benchmark tooling.
- Faust documentation, [Using the Compiler — Vector Code
  Generation](https://faustdoc.grame.fr/manual/compiler/#vector-code-generation):
  loop separation as a way to expose auto-vectorization.
- LLVM, [Alias Analysis Infrastructure](https://llvm.org/docs/AliasAnalysis.html):
  alias results, Mod/Ref information, and their use in load motion and memory
  promotion.
- LLVM, [MemorySSA](https://llvm.org/docs/MemorySSA.html): memory def/use
  versions and cached clobber queries.
- LLVM, [Auto-Vectorization](https://llvm.org/docs/Vectorizers.html): loop and
  SLP vectorizers, cost models, runtime alias checks, reductions, and remainder
  handling.
- LLVM, [Loop Fusion](https://llvm.org/docs/LoopFusion.html): legality
  conditions and the documented consequences of lacking a profitability
  model.
- LLVM, [Language Reference — Fast-Math
  Flags](https://llvm.org/docs/LangRef.html#fast-math-flags): separate
  permissions for reassociation, reciprocal transforms, contraction, and
  approximation.
- Halide, [Storage Folding](https://halide-lang.org/docs/_storage_folding_8h.html):
  reducing buffers to circular live windows when access properties permit it.

These sources support the architecture adopted here: retain domain knowledge
early, represent memory effects explicitly, separate legality from cost, and
make relaxed numerical transformations an explicit contract.
