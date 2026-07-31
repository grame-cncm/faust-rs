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

A recent BPF code-generation comparison is a useful example.
Reusing a materialized old recursion value:

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

With the common effect model, extend the proven BPF state-load reuse into a
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

The BPF old-state temporary should remain profitable
even if its arithmetic use count is low: it removes a proven state read and
communicates a stable value across an output store.

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

The robust fix is the explicit proven old-state value in common FIR. Backend
alias facts are an additional opportunity when the ABI genuinely provides
them.

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
- the fixed three-layer calibrated benchmark design defined in
  [section 8.3](#83-performance-evidence), covering recursion, long delays,
  tables, filters, control-rate work, and vector transports;
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

- BPF retains one old-state load and its measured benefit, while adjacent
  negative controls remain unchanged;
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

#### 8.3.1 Define the performance question first

A corpus is calibrated only relative to a stated population and operating
point. Before selecting a DSP, define which claim the result is intended to
support:

- **mechanism claim:** a lowering or optimization is profitable for a
  compiler-relevant shape such as short recurrence, shared expressions, table
  traffic, or vector transport;
- **application claim:** generated code improves over the intended population
  of real DSP applications; or
- **deployment claim:** generated code improves for a known product workload
  mix, on specified target machines and runtime configurations.

These claims require different sampling and aggregation. A collection built
for language semantics or backend feature coverage is a useful performance
sentinel, but it is not automatically a statistical sample of applications or
of optimization mechanisms.

Define an **operating point** as the tuple:

```text
(target CPU, native compiler and flags, backend, precision and FP policy,
 block size, scalar/vector/scheduling mode, control-update scenario)
```

A performance number applies to one operating point. Combining operating
points is valid only when an explicit deployment distribution supplies their
weights. Otherwise report them separately; an unknown usage distribution
cannot be recovered by averaging configurations equally after seeing results.

Every performance-eligible workload must perform deterministic, validated
work; spend most measured time in generated DSP execution rather than setup,
I/O, allocation, or the harness; run long enough to separate candidate effects
from timer noise; and remain portable across the targets to which the claim
applies. These criteria follow the general benchmark-selection principles of
representativeness, meaningful workload, compute profile, and portability used
by [SPEC CPU](https://ftp.spec.org/cpu2026/docs/overview.html#Q24).

#### 8.3.2 Separate coverage, diagnosis, and representativeness

Maintain three logical layers:

1. **Broad sentinel:** all runnable semantic/backend fixtures. It detects
   unexpected movement, missing results, and severe outliers. It is not used to
   tune a profitability model and has no privileged headline mean.
2. **Mechanism characterization:** parameterized, usually synthetic families
   that isolate compiler-relevant factors and include positive, negative, and
   boundary controls. It explains why a transformation wins, loses, or changes
   regime.
3. **Representative applications:** independently written, application-shaped
   workloads selected before measuring the candidate change. It estimates
   transfer beyond synthetic fixtures.

Do not combine these layers by default. Report a family-balanced mechanism
score for compiler calibration and a separately family-balanced application
score for user-facing performance. Giving synthetic probes and applications
one common vote creates an arbitrary result even if the arithmetic is correct.

Programs without a common reference implementation cannot enter a direct
cross-compiler score. Measure them against a pinned version of the same
compiler in a separate longitudinal series, and retain their semantic checks;
do not silently count missing comparisons as neutral performance.

#### 8.3.3 Calibrate in a compiler-relevant feature space

Names, source size, and application labels are weak predictors of generated
runtime behavior. R0 should assign every candidate a deterministic feature
vector derived from signals and FIR, including:

- operation mix by variability class: integer/real arithmetic, casts,
  comparisons, transcendental and foreign calls;
- recurrence count, order, strongly connected component size, dependency-chain
  depth, and state reads/writes per frame;
- fixed/variable delay geometry, table sizes, read/write ratio, and literal,
  affine, masked, or unknown index classes;
- graph width, critical-path length, fan-out, temporary count, live-range sum,
  and estimated peak live values;
- vectorizable operation fraction, recurrence-constrained fraction, lane count,
  loop count, and transport bytes per block;
- input/output channel counts and init-, control-, and sample-rate work;
- hot state, table, temporary, and emitted-code bytes, expressed both absolutely
  and relative to target cache classes; and
- branches, selectivity, effect barriers, and externally visible calls.

Calibrate the corpus against this space, not against a list of familiar names:

1. define the feature ranges and regimes relevant to planned optimizations;
2. create parameterized mechanism families that cross important boundaries;
3. map candidate applications into the same feature space;
4. fill empty high-priority strata and remove accidental over-density;
5. use clustering and cross-version performance-ratio correlation to identify
   redundant candidates, while retaining semantic or adversarial cases for
   their separate purpose; and
6. freeze the resulting manifest, feature snapshot, family assignment, and
   weights before evaluating a candidate optimization.

Clustering is an aid, not the definition of representativeness: two workloads
with similar static features may stress different downstream compiler paths.
Conversely, a parameter sweep remains one mechanism family even if it produces
many source files. Use leave-one-workload-out and leave-one-family-out influence
analysis to ensure that no unintended member can move the headline score by an
unreasonable amount.

The mechanism layer should cover at least:

| Mechanism | Required controlled variation |
| --- | --- |
| Repeated state and local memory value numbering | repeated exact-location reads; disjoint and clobbering writes; literal, dynamic-index, and unknown-call barriers |
| CSE/materialization | cheap/expensive shared expressions; increasing use count; short/long live ranges; profitable and register-pressure-adversarial cases |
| Scheduling/register pressure | narrow/deep and wide/shallow DAGs; increasing independent width; fan-out with late joins; SLP-friendly isomorphic operations |
| Short recurrence state | increasing recurrence order; one/several outputs; fixed/control-rate coefficients; zero-, partial-, and multi-block state validation |
| Delay representation | small fixed delays; both sides of every representation threshold; medium/cache-sized delays; variable and multi-tap reads sharing a carrier |
| Tables/effects | affine/dynamic reads; repeated reads; distinct/mutable tables; must-alias, proven-disjoint, and unknown indices; effect barriers |
| FIR/IIR recognition | increasing dense-filter order; sparse/symmetric forms; increasing recursive-section count; fixed/control-rate coefficients; related outputs |
| Control placement | increasing block-rate coefficient work, with controls unchanged and updated |
| Vector planning/layout | feed-forward, recurrent, and mixed regions; increasing transport count and independent width; working sets crossing cache regimes |
| Code size and call structure | straight-line expansion versus loops/helpers; increasing inline depth; hot/cold helper combinations |

For threshold-oriented parameters, include values below, at, and above the
decision boundary plus logarithmically spaced scale points. Boundary probes are
diagnostic; they do not each earn an independent corpus-level vote.

#### 8.3.4 Select representative applications independently

The application layer should sample workload strata rather than reward known
benchmark behavior. Relevant strata include:

- feed-forward filtering and spectral shaping;
- low-order and high-order recursive filtering;
- long-delay and feedback effects;
- dynamics and nonlinear processing;
- oscillator/synthesis graphs;
- physical or state-space models;
- multichannel routing, mixing, and spatial processing;
- table/wavetable-heavy processing; and
- mixed control-rate/sample-rate applications.

Choose a small fixed number of representatives per stratum, preferably from
independently written and maintained applications. Check that the resulting
feature distribution covers both common central cases and important tails in
state size, graph width, recurrence intensity, control work, code size, and
memory footprint. A workload is not representative merely because it is large
or historically familiar.

Selection must precede the optimization under test. Additions, removals,
family changes, or weight changes require a versioned rationale, an overlap
period reporting both manifests, and an influence analysis. This prevents
composition drift or benchmark-specific tuning from appearing as compiler
progress. [SPEC's prohibition on narrowly targeted “benchmark
specials”](https://www.spec.org/cpu2026/docs/runrules.html#rule_1.4.2) is a
useful policy analogue: an optimization must provide transferable benefit to
independently written code, not recognize members of the suite.

#### 8.3.5 Performance eligibility and weighting unit

Never delete a semantic fixture merely because it is unsuitable for a
performance score. Keep it in the broad sentinel, but give it zero headline
weight when measured time is dominated by the harness, the generated work is
degenerate, the input reaches a compiler shortcut that does not reflect the
intended workload, or its primary purpose is syntax, diagnostics, metadata,
lifecycle, or UI construction.

Move useful micro-cases into a declared mechanism family instead of treating
each file as an application. The **family**, not the generated file or parameter
point, is the default weighting unit. This rule prevents an implementation
detail—splitting one sweep into 20 files—from multiplying that mechanism's
importance by 20.

Performance eligibility also requires successful correctness validation,
finite measurements, a measured-region dominance check, and enough independent
samples to meet the operating point's noise criterion. Report every excluded
case and reason. Never convert an unsupported, failed, non-finite, or noisy
measurement into a ratio of one.

#### 8.3.6 Primary per-workload measure and operating matrix

For a fixed workload and operating point, the primary quantity is elapsed DSP
execution time per processed frame:

```text
t(i, implementation) = measured_compute_time / processed_frames
```

Report `ns/frame`; when reliable hardware counters are available, also report
`cycles/frame`. Throughput in frames/second is the reciprocal. Given reference
`R` and candidate `C`, define one consistently oriented speed ratio:

```text
r_i = t(i, R) / t(i, C) = throughput(i, C) / throughput(i, R)
x_i = ln(r_i)
```

Thus `r_i > 1` and `x_i > 0` mean that the candidate is faster. Log ratios make
reciprocal changes symmetric and turn multiplicative aggregation into addition.
Keep the ratio dimensionless; absolute time or throughput remains necessary to
distinguish a material gain from a large percentage on a nearly empty kernel.

`faustbench` MBytes/s remains useful for compatibility and gives the same
candidate/reference ratio for a fixed DSP. It is not a common measure of
algorithmic work across DSPs because its numerator depends on sample size and
I/O channel count. Do not sum MBytes/s or compare their absolute values across
different channel shapes.

Secondary explanatory metrics—retired instructions, loads/stores, cache and
branch misses, code size, vector width, FIR operation counts, and energy per
frame—must be normalized and reported separately. They diagnose why time moved;
none is a universal replacement for measured time per frame.

The current developer entry point remains:

```sh
make -C tests/impulse-tests bench \
  BENCH_OPTIONS="-double -run 5 -bs 512"
```

For qualification, measure separate operating points covering at least:

- a one-frame call, a common low-latency block, and a throughput-oriented block;
- every supported precision relevant to the optimization;
- controls held constant and controls updated;
- applicable scalar scheduling and certified vector configurations;
- a strict native optimization lane and the normal production-performance
  flags; and
- at least one target from each CPU/ABI class claimed by the change.

Do not average these operating points unless a deployment profile defines their
weights. Record every result even when it does not enter a headline score.

#### 8.3.7 Aggregation and the optional single score

There is no context-free “average DSP performance.” The correct aggregate
depends on the question.

For normalized candidate/reference ratios with no known deployment frequency,
use a weighted geometric mean. It is invariant to the arbitrary units and
normalization baseline and respects reciprocal comparison. The classic
[Fleming–Wallace result](https://doi.org/10.1145/5666.5673) shows why an
arithmetic mean of normalized ratios does not have these properties.

To prevent family-size bias, aggregate hierarchically. For workload `i` in
family `f`, let `v_fi` be fixed within-family weights that sum to one and let
`W_f` be fixed family weights that also sum to one:

```text
family_log_ratio[f] = sum_i(v_fi * x_i)
corpus_log_ratio    = sum_f(W_f * family_log_ratio[f])
G                   = exp(corpus_log_ratio)
delta_percent       = (G - 1) * 100
```

`G` is the **family-balanced geometric-mean speedup**. Equal family weights and
equal weights within each family are the default when no defensible external
distribution exists. A parameter sweep still owns one family weight; adding
more scale points only subdivides that weight. Weights are part of the corpus
specification and must be frozen before results are inspected.

This default score means “balanced across the declared compiler-relevant
families,” not “expected speedup for the average Faust user.” Only an empirical
usage/deployment distribution can support the latter interpretation.

Publish weight-concentration and influence diagnostics with the manifest:

- maximum family and workload weight;
- effective weighted workload count
  `N_eff = 1 / sum_{f,i}((W_f * v_fi)^2)`;
- maximum change in `G` when one non-mandatory workload or one family is
  removed; and
- sensitivity of `G` to the declared alternative weighting profiles.

These are calibration diagnostics, not extra optimization scores. A nominally
large corpus with a small `N_eff`, or a score controlled by one family, is not
well balanced.

Use another aggregate only when its semantics match a real workload:

- for a known mix executing `a_i` frames of each workload, report the
  actual total-time speedup
  `sum_i(a_i * t(i,R)) / sum_i(a_i * t(i,C))`;
- a harmonic mean of rates is meaningful only when every rate measures the
  same work unit and the mix assigns equal amounts of that work; and
- an arithmetic mean of percentage speedups, a sum of rates, or a ratio built
  from unrelated absolute throughputs has no useful default interpretation.

Do not combine mechanism and application layers into one number. The preferred
headline pair is:

1. family-balanced mechanism speedup, for cost-model calibration; and
2. family-balanced application speedup, for transfer to real programs.

If a product owner supplies explicit probabilities for mechanism/application
layers, scenarios, or operating points, the same weighted-log formula may form
a deployment score. Label it with that profile; it is not a universal compiler
score.

A single value is never sufficient evidence. Publish `G` with a confidence
interval, every family score, the median workload ratio, lower-tail and worst
named ratios, win/loss/tie counts, excluded-status counts, and the number of
regressions beyond the declared practical threshold. A positive `G` cannot
justify an unexplained severe family regression.

#### 8.3.8 Quantitative measurement tools

No single tool answers whether a Signal-to-FIR optimization is profitable and
why. Use one primary timing measurement, then add counters and static evidence
only on representative cases or regressions. The layers below answer different
questions and must not be substituted for one another.

| Layer | Preferred tools | Quantities to retain | Proper use and limitation |
| --- | --- | --- | --- |
| Generated-DSP throughput | `faustbench -single` through `make bench` | raw duration, frames, calls, `ns/frame`, optional `cycles/frame`, MBytes/s, candidate/reference ratio | Primary end-to-end runtime result for the generated `compute`; MBytes/s is comparable between two implementations of the same DSP, not as a measure of algorithmic work across different DSPs |
| Linux hardware counters | [`perf stat`](https://man7.org/linux/man-pages/man1/perf-stat.1.html), followed by `perf record/report` when attribution is needed | task-clock, cycles, instructions, branches, branch misses, cache references/misses, migrations, context switches; all relevant events normalized per processed frame | Explains real hardware behavior; event names, availability, multiplexing, and counter semantics vary by CPU, so retain the raw event list and `perf` warnings |
| macOS hardware and sampling profiles | Instruments Time Profiler and CPU Counters, recorded/exported with [`xctrace`](https://developer.apple.com/documentation/xcode/xcode-command-line-tool-reference) | sampled hot functions, CPU time, available retired-instruction/cycle/cache counters, thread/core placement, thermal observations | Native Apple platform diagnosis; counter sets depend on the processor and Instruments version, and sampling percentages are attribution rather than elapsed-time measurements |
| Native compiler decisions | Clang/LLVM [`-Rpass`, `-Rpass-missed`, `-Rpass-analysis`, and `-fsave-optimization-record`](https://llvm.org/docs/Remarks.html) | vectorized/missed loops, vector width and interleave decisions when reported, inlining, spills/stack-size remarks, stable missed-reason text | Shows what the downstream optimizer decided; a “passed” remark does not establish that the resulting program is faster |
| Machine code | [`llvm-objdump`](https://llvm.org/docs/CommandGuide/llvm-objdump.html), platform disassembler, binary size tools | hot-loop instructions, loads/stores, calls, branches, vector width, stack frame, function and text size | Confirms that intended FIR facts survived lowering; static instruction counts are not dynamic execution counts |
| Static CPU model | [`llvm-mca`](https://llvm.org/docs/CommandGuide/llvm-mca.html) on an isolated hot-loop assembly region | predicted cycles/iteration, block reciprocal throughput, IPC, resource pressure, register-file pressure and timeline | Diagnoses dependency/resource bottlenecks for CPUs with an LLVM scheduling model; it does not model the real cache hierarchy or branch prediction and is not a runtime benchmark |
| Deterministic instruction/cache simulation | [`Cachegrind`](https://valgrind.org/docs/manual/cg-manual.html) and `cg_diff`, on supported hosts | executed instructions, data reads/writes, simulated I1/D1/last-level misses and branch mispredictions | Useful for deterministic before/after attribution; simulated cache and predictor results are not hardware cycles and must not replace `perf`/Instruments timing |
| Whole-command and compile-time exploration | [`hyperfine`](https://github.com/sharkdp/hyperfine) plus the mandatory `compile-budget-check` | individual wall times, mean/median/spread, warmup count, exported JSON/CSV | Suitable for compiler invocations or wrapper-level experiments; process startup makes it unsuitable for measuring a tiny `compute` kernel directly |

`faustbench` currently reports an intentionally optimistic throughput estimate:
each internal measurement is the mean of the 50 fastest intervals, and
`-run N` returns the fastest outer run. Preserve that number for continuity,
but extend the harness before using it as a performance gate: retain every
outer-run value and enough interval statistics to compute a central estimate
and dispersion. “Best observed” answers what the code can reach under favorable
conditions; median paired performance answers whether the new generator is
reliably faster.

The common FIR observation pass from R0 is another measurement instrument. Its
deterministic counts—operations, calls, state/table loads and stores, temporary
live ranges, estimated peak live values, loop/transport counts, state bytes,
and emitted code size—must be stored next to runtime results. These counts are
especially useful when hardware counters are unavailable, but they remain a
model until correlated with native code and elapsed time.

#### 8.3.9 Required numeric record

For every DSP/configuration pair, retain a machine-readable record containing:

- corpus/layer version, family, within-family and family weights, DSP source
  hash, compiler commits, complete flags, block size, precision,
  scheduling/vector mode, native compiler, target triple, CPU model, operating
  system, and date;
- every raw elapsed-time or throughput sample and its execution order;
- frames and `compute` calls, from which `ns/frame` and `ns/call` are derived;
- paired candidate/reference throughput ratio and log-ratio;
- median, median absolute deviation or another declared robust spread, and a
  confidence interval for the ratio;
- R0 FIR metrics, generated source size, native text/function size, and a hash
  of the binary or disassembly used for diagnosis;
- requested and actually counted hardware events, normalized per frame, plus
  counter multiplexing or unsupported-event diagnostics; and
- correctness checksum/status, non-finite status, and any measurement failure.

Prefer `ns/frame` or `cycles/frame` as the architecture-level primary unit.
Retain MBytes/s for compatibility with Faust tooling and user familiarity.
Never compare absolute MBytes/s between DSPs with different channel counts as
if it measured equal algorithmic work. Normalize secondary metrics explicitly,
for example `instructions/frame`, `branch-misses/million instructions`, and
`LLC-misses/1000 frames`; raw percentages without their denominator are not
auditable.

#### 8.3.10 Repetition, uncertainty, and decision protocol

Use a paired protocol because both generators run on the same machine:

1. build both variants before timing with identical architecture, native
   compiler, flags, target, and link mode;
2. verify output/final-state correctness and ensure output stores remain
   observable before measuring;
3. warm the code and data, then interleave the two variants per DSP using a
   balanced `ABBA`/`BAAB` or seeded-random order rather than running all of one
   compiler first;
4. run at least five independent outer-process pairs for routine screening;
   use a pilot variance decomposition to choose enough process/build
   repetitions for a small or release-note-worthy effect;
5. retain all samples and mark interruptions, thermal throttling, context
   switches, migrations, or counter multiplexing instead of silently deleting
   inconvenient runs;
6. calculate paired log-ratios, estimate one robust central log-ratio and a 95%
   confidence interval per workload, then aggregate by the fixed weights from
   section 8.3.7; and
7. rerun a named regression with counters and native-code evidence until its
   cause is attributed or explicitly recorded as unresolved.

Run on an otherwise quiet machine. When supported, use one pinned performance
core, a stable performance/frequency policy, mains power, and enough cooldown
to avoid thermal drift; record what was actually controlled. Do not use a
privileged real-time configuration as an undocumented default because it can
make results irreproducible on developer machines and CI.

Treat the independent outer process/build as the sampling unit when it is the
highest repeated level. Calls or timer intervals from one process share code
layout, state, frequency history, and environment and therefore must not be
counted as independent evidence. Follow the variance-level approach advocated
by [Kalibera and Jones](https://kar.kent.ac.uk/33611/): use a pilot to locate
material sources of variation, spend repetitions at those levels, and report an
effect-size confidence interval rather than only a point estimate.

For a score defined on a fixed versioned corpus, compute measurement uncertainty
by resampling or modeling repetitions while keeping workloads and weights
fixed. Resampling workloads answers a different question—generalization to a
larger DSP population—and is justified only when the selection procedure makes
that population interpretation credible. A hierarchical bootstrap may then
resample families, workloads within families, and independent repetitions, but
its wider population claim must be explicit.

Treat a result smaller than the observed noise band as inconclusive. The
current 5% warning threshold is a practical regression triage level, not a
claim that every 4.9% movement is noise. Accept a profitability change only
when its intended mechanism family improves beyond measurement uncertainty, no
application family has an unexplained material regression, and the responsible
FIR/native-code evidence moves in the expected direction. A cleaner generated
C++ source, a lower static instruction count, a vectorization remark, or a
better `llvm-mca` estimate is supporting evidence—not a substitute for measured
paired runtime.

### 8.4 Compilation-cost evidence

Every implementation phase in `transform`, `fir`, `codegen`, or `compiler`
must run:

```sh
cargo run --release -p xtask -- compile-budget-check
```

Optimization analysis must not recreate pairwise or backward-scan complexity
that becomes quadratic on large DSP graphs. Prefer cached def/use facts,
per-object indexing, and linear or near-linear passes.

## 9. Closed-loop improvement toward an empirical fixed point

A recursive improvement loop is feasible, provided that it is treated as a
bounded experiment rather than as a claim that the compiler can optimize
itself to a universal optimum. The loop may propose, implement, measure, and
retain improvements repeatedly, but correctness remains a hard constraint,
the benchmark contract remains fixed during each experiment, and a human
review remains responsible for changes to compiler semantics or optimization
legality.

### 9.1 Define the fixed point precisely

Let `C_k` be the compiler at iteration `k`, `N(C_k)` the bounded set of
admissible candidate compilers that the current search procedure can propose,
and `E` a frozen evaluation contract. `E` contains:

- the versioned DSP manifests, family assignments, eligibility rules, and
  aggregation weights from section 8.3;
- the target machines, native compilers, backend flags, sample types, block
  sizes, and other operating points;
- the required output, final-state, lifecycle, and numerical contracts;
- the compilation-time, generated-code-size, and runtime-memory budgets; and
- the measurement procedure, minimum detectable effect `epsilon`, confidence
  rule, and exploration budget.

An **empirical fixed point for `E` and `N`** is reached when no admissible
candidate found within the declared exploration budget produces a validated
improvement of at least `epsilon` without violating a constraint. It is only a
local statement about the frozen corpus, targets, search neighborhood, and
measurement resolution. It is not proof that no better compiler exists.

The recurrence is therefore an accept-or-stay operation, not an obligation to
change code on every round:

```text
C_(k+1) = select_E(Pareto({C_k} union validated_candidates(N(C_k))))
C_(k+1) = C_k when no candidate satisfies the predeclared acceptance rule
```

`select_E` is a versioned engineering policy over the visible Pareto frontier,
not an after-the-fact choice of whichever scalar score makes one candidate win.

Changing a corpus, family weight, target, backend, native compiler, numerical
contract, or available transformation starts a new evaluation epoch. Thus the
loop has a reproducible stopping condition while remaining able to resume when
new evidence or a new optimization idea appears.

Distinguish three useful stopping claims:

1. a **measurement fixed point**: any remaining gain is smaller than the
   experiment can reliably distinguish from noise;
2. a **search-space fixed point**: no improving candidate was found in the
   explicitly bounded neighborhood and budget; and
3. an **engineering fixed point**: the expected gain no longer justifies the
   implementation, verification, compile-time, or maintenance cost.

Every report must name which claim it makes. "No further improvement" without
that qualification is not an acceptable conclusion.

### 9.2 Use nested loops with different authority

The process should contain four loops rather than one unrestricted agent:

1. The **measurement loop** rebuilds one candidate, runs semantic guardrails,
   executes paired benchmarks, and produces uncertainty-aware metrics.
2. The **profitability loop** searches thresholds, cost weights, legal plan
   choices, and pass enablement among transformations whose correctness
   conditions are already encoded and tested.
3. The **compiler-development loop** adds an analysis or transformation from
   this roadmap, together with its legality model, provenance, tests, and
   compilation-cost evidence. This loop requires code review; search is not
   allowed to invent or relax legality conditions.
4. The **corpus-governance loop** revisits coverage, redundancy, applications,
   and weights only between evaluation epochs. It must not change the scoring
   rule in response to a candidate being evaluated.

The first two loops can become highly automated. The third can use automated
patch proposals but retains explicit engineering approval. The fourth is a
benchmark-design activity, independent of whether the current compiler wins
or loses.

The resulting control flow is:

```text
freeze epoch -> observe -> propose -> verify semantics -> measure development set
             -> validate holdout -> accept/reject -> record -> repeat or stop
```

Any semantic failure, nondeterministic output, undefined-behavior finding, or
budget violation terminates evaluation of that candidate. Correctness is not a
negative term in a reward function that a sufficiently large speedup can
outweigh.

### 9.3 Freeze an epoch and separate development from confirmation

At the start of an epoch, write an immutable manifest containing the complete
evaluation contract `E`, the compiler and dependency revisions, the reference
compiler, random seeds, run-order policy, machine description, and time/search
budget. Retain a stable anchor compiler for the whole epoch in addition to the
immediate parent of each candidate. Parent comparisons decide whether a patch
is locally beneficial; anchor comparisons expose cumulative drift hidden by a
long chain of individually small changes.

Partition evidence by purpose:

- a **development set** of parameterized mechanism families drives diagnosis
  and candidate search;
- an independently selected **validation set** of representative applications
  determines whether the mechanism generalizes under the declared use case;
  and
- a small **shadow set**, not consulted during search, confirms the selected
  candidate before an epoch is closed or a release claim is made.

Keep performance data separate from the semantic differential corpus: all
eligible performance cases still pass correctness gates, but correctness
coverage should not be reduced merely because a case is redundant for timing.
Likewise, a broad sentinel may detect an unexpected regression without being
given equal optimization weight.

Repeatedly inspecting validation failures and adapting the compiler to them
turns the validation set into another development set. Record every
consultation. Once it has influenced a proposal, either treat it as development
data or reserve a fresh versioned holdout for final confirmation. Rotate or
expand holdouts only at an epoch boundary, never silently during a run.

### 9.4 Generate candidates at controlled levels

Introduce automation in increasing order of semantic risk:

1. **Parameter search** tunes numerical profitability thresholds and cost
   weights within documented ranges.
2. **Plan search** selects or orders already verified lowering alternatives,
   such as materialize versus recompute or legal loop plans.
3. **Transformation proposals** modify effect analysis, IR rewrites, or state
   layout and therefore require an explicit legality argument, focused tests,
   differential oracles, and human review before measurement can authorize
   retention.
4. A future **learned policy** may replace part of a cost heuristic only after
   enough diverse, versioned observations exist. Its feature schema, training
   corpus, model revision, confidence behavior, and deterministic fallback
   must be part of the compiler artifact.

Grid, Bayesian, evolutionary, portfolio, or learned search can all propose
candidates. The proposal algorithm does not change the acceptance contract.
In particular, the search must choose among legal actions exposed by the
compiler rather than learn whether a semantically unsafe action happens to
pass the current corpus. This separation follows the architecture used by
[LLVM MLGO](https://www.llvm.org/docs/MLGO.html): the compiler owns correctness
and exposes optimization decisions to an external training or policy process.

Keep candidate patches small enough to attribute their effect. If a candidate
combines independent transformations, rerun an ablation or factorial subset so
that a later iteration does not preserve dead complexity merely because it was
bundled with a winning change.

### 9.5 Optimize a constrained vector, not one opaque score

The family-balanced geometric runtime score from section 8.3.7 is a useful
summary, but it is not a complete objective. Evaluate a candidate as a vector
such as:

```text
J(C) = (
    application log-speedup by operating point,
    mechanism log-speedup by family,
    compilation cost in calibration units,
    generated-code and binary size,
    runtime memory
)
```

Semantic parity, final-state identity, lifecycle conformance, determinism, and
the absence of known undefined behavior are hard constraints outside `J`.
Maintain a Pareto frontier when runtime, compilation cost, size, and memory
trade off. A scalar utility may prioritize which candidate to measure next,
but must not conceal those tradeoffs or override a guardrail.

Accept a candidate only when all of the following hold:

1. all semantic and structural checks pass;
2. the intended mechanism family improves by at least the predeclared
   practical threshold, with the required confidence;
3. the validation application score meets its predeclared non-regression or
   improvement criterion at every claimed operating point;
4. no family or influential workload has an unexplained material tail
   regression;
5. compilation cost, size, and memory remain inside their budgets; and
6. a fresh independent confirmation run reproduces the result.

An application-neutral infrastructure patch may be retained without an
immediate runtime win only if that exception was declared before measurement
and it demonstrably improves analysis quality, observability, or a later
phase's prerequisites without regressing the budgets.

### 9.6 Control adaptive-search bias

Measuring many candidates and retaining the largest observed speedup selects
positive noise as well as good code. This winner's curse grows with the number
of variants tried, even if every individual confidence interval looks
reasonable. Therefore:

- use development measurements for search and the holdout only for
  confirmation;
- log every attempted candidate, including failures and inconclusive results;
- require a fresh build and independent measurement of the selected candidate;
- use the paired and nested-repetition method from section 8.3.10;
- predeclare `epsilon`, budgets, stopping rules, and the maximum search effort;
- inspect family and tail results rather than accepting on the aggregate alone;
  and
- do not update a baseline, drop a workload, or change a weight to turn a
  failed candidate into a pass.

When the search is large, use sequential or multiple-comparison-aware methods
designed for adaptive experiments, or treat all search-stage statistics as
ranking heuristics and reserve inferential claims for the untouched
confirmation set. A complete negative ledger also prevents future agents from
retesting the same noisy or invalid idea without new evidence.

### 9.7 Make the loop replayable before making it autonomous

A future `xtask optimize-loop` could orchestrate the process without granting
it authority to merge code. Its durable inputs and outputs should include:

- an epoch manifest and candidate specification;
- isolated compiler builds and hashes of measured binaries;
- semantic, structural, compilation-budget, and performance results;
- raw samples, counter data, generated-code evidence, and environment metadata;
- the patch, parent, stable anchor, decision, and rejection reason; and
- a deterministic replay command for each accepted or shortlisted candidate.

Candidate states should be explicit—for example `invalid-semantics`,
`over-budget`, `inconclusive`, `pareto-candidate`, `rejected-validation`, and
`accepted-proposal`. The tool may create a reviewable proposal branch, but it
must never automatically merge, mutate the frozen corpus, relax a gate, or
rewrite a baseline. CompilerGym's emphasis on reproducible compiler
environments and fault detection, and OpenTuner's separation of the search
technique from the measurement interface, are useful models for this layer.

The first implementation should use append-only JSON or JSONL records plus a
human-readable decision report. A database and distributed workers are useful
only after the replay contract is stable. Cache generated sources and builds
only when their keys include every semantic and measurement-relevant input;
otherwise cache reuse can compare the wrong compilers.

### 9.8 Budget the measurement funnel

The closed loop is useful only if its cost permits enough candidate diversity.
Do not run the complete release-quality experiment after every proposal. Model
the lower bound on pure timed-kernel work before freezing an epoch. For a
paired candidate/baseline experiment, define:

- `n` as the number of timing-eligible workloads;
- `p` as the number of operating points;
- `o` as the number of independent outer process/build pairs;
- `r_c`, `r_b` as the inner runs for candidate and baseline; and
- `d_c`, `d_b` as the timed duration of one corresponding run.

The irreducible timed work is then:

```text
T_kernel = n * p * o * (r_c * d_c + r_b * d_b)
```

Add compiler execution, native compilation, process startup, cache
preconditioning, cooldown, semantic gates, profiling, failures, and report
generation to obtain elapsed wall time. Record both values: `T_kernel` makes
the experiment design auditable, while actual elapsed time is needed for
capacity planning.

The pinned C++ reference (`8eebea429`,
`tools/benchmark/faustbench.cpp`) constructs its `faustbench` measurement with
a five-second interval for one run. With symmetric `-run 5`, comparing two
compiler revisions therefore costs at least 50 timed seconds per eligible
workload and operating point for one outer pair:

```text
2 revisions * 5 runs * 5 seconds = 50 seconds/workload/operating-point
```

For scale, a frozen 91-workload manifest requires 4,550 seconds, or 75 minutes
50 seconds, for one full outer pair. Five independent outer pairs require
22,750 seconds, or about 6 hours 19 minutes. A 135-workload manifest would
require about 1 hour 52 minutes per pair and 9 hours 23 minutes for five pairs.
These are lower bounds for one operating point; additional block sizes,
precisions, targets, or backends multiply them. Failed or ineligible workloads
may finish early, but must not be assumed as a permanent time saving.

Use a staged funnel with predeclared promotion rules:

| Stage | Purpose | Typical design | Approximate single-machine elapsed time |
| --- | --- | --- | --- |
| Semantic preflight | Reject invalid candidates before timing | Full correctness/golden subset required by the touched phase; no runtime benchmark | Depends on touched crates; cache reference artifacts safely |
| Mechanism screening | Rank many candidates and expose the intended FIR effect | 6–12 diagnostic workloads, one operating point, `run = 1`, one outer pair | 3–10 minutes including ordinary build overhead |
| Qualification | Reject fragile or family-specific apparent wins | 20–30 stratified workloads, `run = 2..3`, approximately three outer pairs | 30–90 minutes |
| Confirmation | Authorize an accepted proposal or close an epoch | Complete eligible validation/shadow manifest, `run = 5`, at least five outer pairs | Approximately 6–10 hours at the 91–135 workload scale, per operating point |
| Attribution | Explain a finalist, regression, or model error | Counters, remarks, disassembly, and cache/static analysis on affected families only | Variable; never charge this to every search candidate |

The ranges are planning estimates, not substitutes for a pilot. Measure actual
build, gate, timing, and cooldown latency in L0, then version the resulting
cost model in the epoch manifest. Choose the screening sample and search budget
from that model. For example, screening 30–100 candidates at 3–10 minutes each
consumes roughly 1.5–17 machine-hours before qualification; confirming three
to five finalists adds roughly 18–50 machine-hours at one operating point.

Inner-loop performance comparisons should normally run `faust-rs` candidate
against its immediate `faust-rs` parent. Compare against the stable epoch
anchor periodically and at confirmation. C++ Faust remains the independent
semantic oracle and an important external performance reference, but rerunning
its runtime benchmark for every profitability candidate answers a different
question and doubles work that may not guide the candidate decision. Cache an
oracle artifact only when its compiler revision, source, libraries, options,
and target are all in the cache key.

Do not execute competing timed candidates concurrently on the same physical
machine. That increases throughput at the cost of correlated contention and
invalid comparisons. Identical dedicated workers may evaluate different
candidates in parallel when each worker keeps its candidate/baseline pairing
local; record worker identity and model host effects. Heterogeneous machines
define distinct operating points rather than interchangeable workers.

Under the assumptions of one experienced compiler developer assisted by an
agent and one dedicated measurement machine, use the following provisional
calendar budget:

| Milestone | Planning range from the current infrastructure level |
| --- | --- |
| L0/L1 manifests, two-revision runner, ledger, replay, and reports | 2–4 weeks |
| First bounded parameter/verified-plan epoch after L1 | 1–2 additional weeks |
| Empirical fixed point for one low-to-medium-risk optimization family | 4–8 weeks total |
| Several low-to-medium-risk roadmap families | 3–6 months |
| Serious exploration including state promotion, layout, vector planning, and specialization | 6–18 months |

These ranges estimate engineering convergence, not uninterrupted CPU time and
not a delivery promise. Additional homogeneous workers shorten independent
measurements, but not legality design, debugging, attribution, review, or the
need to wait for a fresh confirmation. Re-estimate after L1 using observed
candidate throughput, variance, rejection rates, and the fraction promoted to
qualification.

### 9.9 Stop, audit, and restart deliberately

Close an epoch only after all predeclared conditions are satisfied. A practical
rule is:

- the candidate queue and exploration budget are exhausted;
- no fresh candidate has produced a confirmed gain of at least `epsilon` for
  `K` consecutive independent proposal rounds;
- remaining cost-model errors or missed opportunities no longer decrease on
  the development families;
- the best candidates either fall below the minimum detectable effect or
  violate a declared tradeoff; and
- the validation and shadow results show no unresolved regression.

Choose `epsilon`, `K`, and the exploration budget from pilot variance and the
engineering cost of a false decision, then record them in the epoch manifest.
Do not choose them after seeing the result. Close-out should include a Pareto
frontier, accepted and rejected ideas, residual FIR/native-code pathologies,
coverage gaps, and the exact fixed-point claim from section 9.1.

Restart with a new epoch when a target or dependency changes, a new DSP family
reveals a coverage hole, a new FIR fact becomes available, a new transformation
class enters the neighborhood, or measurement precision materially improves.
The old epoch remains valuable historical evidence; it must not be silently
reinterpreted under the new contract.

### 9.10 Stage the automation

Implement the loop in reviewable phases:

| Stage | Automation | Pass criterion |
| --- | --- | --- |
| L0 | Frozen manifests, append-only candidate ledger, and manual proposals | An independent developer can replay one decision from raw artifacts |
| L1 | Automatic build, semantic gates, paired measurement, and decision report | Repeated runs classify known wins, losses, and noise consistently |
| L2 | Bounded parameter and verified-plan search | Search cannot bypass legality or resource budgets; holdout remains untouched until confirmation |
| L3 | Automated source-patch proposals from roadmap tasks | Every patch carries an analysis/test obligation and requires human review before retention |
| L4 | Optional learned profitability policy | Versioned data/model/features, deterministic fallback, cross-target validation, and measurable advantage over the explicit heuristic |

Do not begin at L3 or L4. The highest-leverage first step is to make L0/L1
boring and auditable: the same compiler, corpus, and machine description must
produce the same eligibility decisions and statistically compatible results.
Only then can recursive optimization accumulate evidence instead of
accumulating benchmark-specific accidents.

## 10. Priorities at a glance

| Priority | Work | Expected leverage | Correctness risk | Main evidence |
| --- | --- | --- | --- | --- |
| P0 | Metrics and decision reports | Enables every later decision | Low | Output identity, compile budget |
| P0 | Closed-loop manifests, replay, and candidate ledger | Makes iterative decisions cumulative and auditable | Low | Independent replay, frozen holdout |
| P1 | Shared effect/location model | Unlocks safe memory optimization | Medium | Mutation tests, analysis cross-check |
| P1 | Costed CSE/materialization | Broad scalar/all-backend improvement | Low–medium | State-reuse family + calibrated corpus |
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

## 11. External references

The references below are design analogues, not specifications for Faust:

- Faust documentation, [Optimizing the Code](https://faustdoc.grame.fr/manual/optimizing/):
  variability tiers, scalar/vector code shapes, and benchmark tooling.
- Faust documentation, [Using the Compiler — Vector Code
  Generation](https://faustdoc.grame.fr/manual/compiler/#vector-code-generation):
  loop separation as a way to expose auto-vectorization.
- SPEC CPU, [benchmark selection
  overview](https://ftp.spec.org/cpu2026/docs/overview.html#Q24) and [run and
  reporting rules](https://www.spec.org/cpu2026/docs/runrules.html):
  representativeness, portability, reproducible conditions, geometric-mean
  ratio metrics, and safeguards against benchmark-specific optimizations.
- Fleming and Wallace, [How Not to Lie with Statistics: The Correct Way to
  Summarize Benchmark Results](https://doi.org/10.1145/5666.5673): properties
  that select the geometric mean for normalized benchmark ratios and the need
  to report dispersion and extremes.
- Kalibera and Jones, [Rigorous Benchmarking in Reasonable
  Time](https://kar.kent.ac.uk/33611/): variance across nested experimental
  levels and effect-size confidence intervals.
- PARSEC, [benchmark-suite characterization and architectural
  implications](https://collaborate.princeton.edu/en/publications/the-parsec-benchmark-suite-characterization-and-architectural-imp/):
  characterizing workload diversity rather than relying only on application
  labels.
- LLVM, [Machine Learning Guided Optimization
  (MLGO)](https://www.llvm.org/docs/MLGO.html): separating compiler-owned
  correctness from externally trained optimization decisions, with versioned
  features and policies.
- Cummins et al., [CompilerGym: Robust, Performant Compiler Optimization
  Environments](https://arxiv.org/abs/2109.08267): reproducible compiler
  optimization environments, large search spaces, datasets, and fault
  detection.
- Ansel et al., [OpenTuner: An Extensible Framework for Program
  Autotuning](https://commit.csail.mit.edu/papers/2014/ansel-pact14-opentuner.pdf):
  decoupling measurement from a portfolio of search techniques and supporting
  multi-objective autotuning.
- Fursin et al., [Collective Mind, Part II: Towards Performance- and
  Cost-Aware Software Engineering as a Natural Science](https://arxiv.org/abs/1506.06256):
  versioned experimental workflows, shared observations, and reproducible
  exploration of optimization choices.
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
early, represent memory effects explicitly, separate legality from cost,
calibrate and aggregate performance evidence before tuning profitability, make
relaxed numerical transformations an explicit contract, and keep recursive
search inside a frozen, replayable, multi-objective evaluation contract.
