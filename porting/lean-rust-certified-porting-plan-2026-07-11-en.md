# Lean/Rust Certified Porting Plan and Canonical Certificate Boundary

Status: normative porting plan; implementation in progress. The P4-P6
producer/checker chain and its production integration through P6.6 were
delivered on 2026-07-14. Canonical serialization, Lean cross-checking, and the
R5 semantic relation remain open.

Date: 2026-07-11.

Related documents:

- [`vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`](./vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md)
- [`vector-mode-scheduling-formal-spec.lean`](./vector-mode-scheduling-formal-spec.lean)
- [`schemas/vector-verification-certificate-v1.schema.json`](./schemas/vector-verification-certificate-v1.schema.json)

## 1. Objective

Connect the progressive Rust port to the Lean specification so that each
critical compiler phase produces a finite result that can be checked
independently before the next phase consumes it.

The target is a high-assurance, fail-closed compiler pipeline:

```text
untrusted/complex producer
    -> result + canonical witness
    -> small independent verifier
    -> certified result or typed internal error
```

"Untrusted" here is an architectural term. It does not imply low-quality Rust;
it means correctness does not depend solely on the producer implementation.

Absolute security is not a realistic claim. The remaining trusted computing
base includes the specification, Lean kernel and runtime, serialization layer,
Rust toolchain, backend/runtime code, foreign functions, operating system, and
hardware. The achievable goal is to make structural compiler errors detectable
at narrow boundaries and to reduce semantic trust through refinement proofs and
differential execution.

## 2. Assurance Model

Use four explicit assurance levels.

| Level | Meaning | Required evidence |
|---|---|---|
| L1 - tested | Conventional implementation confidence | Rust unit, property, corpus, and C++ differential tests |
| L2 - runtime certified | A phase result is rejected unless a Rust verifier accepts it | Canonical artifact plus independent Rust checker |
| L3 - Lean checked | The Rust artifact is also accepted by the executable Lean reference checker | Cross-language artifact parity in CI |
| L4 - refinement proved | A selected Rust implementation is connected deductively to the Lean definition | Machine-checked refinement theorem for the supported Rust subset |

Required rollout policy:

- every new scheduler and vector-planning phase reaches L2 before activation;
- every versioned acceptance corpus reaches L3 before the corresponding phase
  gate is considered complete;
- L4 starts with small pure verifiers and analysis functions, not backends;
- a lower level must never be described as a proof of a higher level.

Normative terminology:

- **formally specified** means Lean contains a typed definition of the contract;
- **kernel checked** means Lean accepted a concrete theorem or executable guard;
- **runtime certified** means the independent Rust checker accepted one result;
- **translation validated** means independent executions agreed on the tested
  observations;
- **formally verified** is reserved for a stated refinement theorem whose
  assumptions and trusted computing base are named.

The current companion Lean file is a kernel-checked specification with selected
finite theorems and guards. It is not a formally verified faust-rs compiler.

## 3. Trust Boundaries

### 3.1 Producer/checker separation

The scheduling algorithm, vector-plan builder, and FIR router may use complex
data structures and optimizations. Their checkers must instead use direct,
obviously terminating traversals over canonical finite snapshots.

The checker must not call the producer algorithm. In particular:

- `verify_decorations` checks exact prepared-signal coverage and consistency of
  type, variability, clock, effect, occurrence, delay, and labelled dependency
  facts without rebuilding vector placement;
- `verify_schedule` checks permutation coverage and every edge order; it does
  not rerun DFS/BFS/Special/Reverse-BFS;
- `verify_vector_plan` reconstructs induced epoch graphs and checks ownership,
  endpoints, transports, barriers, and vector-safety evidence;
- `verify_routed_fir` reconstructs region/type/store/load/effect facts from the
  routed FIR snapshot rather than trusting planner booleans.

### 3.2 Lean as normative semantics

[`vector-mode-scheduling-formal-spec.lean`](./vector-mode-scheduling-formal-spec.lean)
defines the normative mathematical meaning of the finite checks. Executable
Lean functions are the reference oracle. Proposition-valued interfaces state
the remaining proof obligations.

The Lean `Scheduler.sound`/`complete` structure is an optional L4 refinement
target for the producer, not a Rust runtime requirement. Progressive Rust work
uses the L2 path: an ordinary scheduler proposes an order, and the independent
checker constructs a certificate or rejects it. The illustrative Lean
`Expr`/`HasType` subset is connected to production only through decoration
records exported from the real prepared `SigId` forest; it must not evolve into
an untested shadow AST.

The canonical JSON schema is not the semantics. It validates syntax, shape,
closed enums, integer ranges, and required fields. Semantic verification still
has to check uniqueness, hashes, graph order, ownership, completeness, effects,
and simulation premises.

### 3.3 Fail-closed behavior

A failed check must produce a typed compiler-internal diagnostic and stop before
the next trust boundary. No backend may receive FIR derived from a rejected
vector plan. Release builds may disable expensive Lean subprocess checks, but
must retain the L2 Rust checkers for all certificate-gated phases.

## 4. Canonical Artifact Schema

The normative machine-readable schema is:

```text
porting/schemas/vector-verification-certificate-v1.schema.json
```

It defines five top-level artifact kinds:

1. `decoration_certificate`;
2. `schedule_certificate`;
3. `vector_plan_certificate`;
4. `routed_fir_certificate`;
5. `verification_result`.

The first four contain claims to be checked. They are not valid merely because
they satisfy JSON Schema. `verification_result` records a named verifier's
acceptance or rejection of the relevant canonical subject hash: `graph_hash`,
`decoration_hash`, `plan_hash`, or `routed_fir_hash`.

### 4.1 Versioning

Every artifact contains:

```json
{
  "schema_version": 1,
  "artifact_kind": "schedule_certificate",
  "producer": {
    "name": "faust-rs",
    "version": "0.1.0",
    "git_commit": "0123456789abcdef0123456789abcdef01234567"
  },
  "program": {
    "case_id": "tests/corpus/example.dsp",
    "source_sha256": "...64 lowercase hexadecimal digits..."
  }
}
```

Rules:

- `schema_version` changes only for an incompatible representation or semantic
  interpretation;
- additive fields require a new schema version because all v1 objects reject
  unknown properties;
- readers reject unknown versions and artifact kinds;
- converters between versions are explicit tools, never implicit parser
  fallbacks;
- repository-relative `/`-separated `case_id` values are required for portable
  snapshots; absolute paths are forbidden.

### 4.2 Canonical JSON encoding

JSON Schema does not define byte identity. The following rules are therefore
normative for hashing and committed snapshots:

1. encode UTF-8 without a byte-order mark;
2. use RFC 8785 JSON Canonicalization Scheme object-key ordering and scalar
   rendering;
3. emit no insignificant whitespace for hash input;
4. terminate committed pretty-printed files with one LF;
5. use non-negative integers no greater than `2^53 - 1`;
6. do not encode integers as floating-point values or strings;
7. reject duplicate object keys before canonicalization;
8. reject unknown fields before hashing;
9. use lowercase hexadecimal SHA-256 strings;
10. normalize no user string implicitly: strings are hashed as supplied.

The Rust implementation should use a dedicated canonical serializer rather than
assuming `serde_json::to_string` is canonical. The Lean importer must parse the
same JSON value and independently recompute every declared hash.

### 4.3 Array-order rules

Arrays representing mathematical sets have one required order:

| Array | Canonical order |
|---|---|
| graph `nodes` | ascending node id |
| graph `edges` | `(consumer, dependency, kind)` ascending |
| decoration `signals` | ascending signal id |
| decoration `dependencies` | `(consumer, dependency, kind, delay)` ascending |
| plan `signals` | ascending signal id |
| plan `loops` | ascending loop id |
| plan `epochs` | `(rank, id)` ascending |
| epoch `loops` | ascending loop id |
| plan `transports` | ascending transport id |
| plan data/effect edges | `(consumer, dependency, kind)` ascending |
| effects on one signal | semantic source order, not sorted |
| loop roots | deterministic materialization/source order |
| schedule `ordered_nodes` | execution order, never sorted |
| routed FIR statements | emitted execution order, never sorted |

The verifier rejects noncanonical set ordering even when the represented set is
equivalent. This makes byte snapshots stable and removes accidental `HashMap`
iteration from the compatibility surface.

### 4.4 Edge convention

Every edge is encoded as:

```json
{
  "consumer": 7,
  "dependency": 3,
  "kind": "data"
}
```

It means `7 -> 3`: node 7 consumes node 3, so node 3 must execute first. No
field named `from` or `to` is permitted because those names have repeatedly
caused direction ambiguity.

The schedule condition is:

```text
position(edge.dependency) < position(edge.consumer)
```

### 4.5 Hash projections

Hashes bind a certificate to the exact object it checks.

```text
decoration_hash = SHA256(canonical_json(decorations))

graph_hash = SHA256(canonical_json(graph))

plan_hash = SHA256(canonical_json(plan))

routed_fir_hash = SHA256(canonical_json({
    "plan_hash": plan_hash,
    "routed_fir": routed_fir
}))
```

The hash field itself, producer metadata, program metadata, schedules, and
verification results are excluded from these projections. `VectorPlan` has no
scheduling strategy or selected loop order, so changing `-ss` must leave
`plan_hash` byte-identical.

### 4.6 Schedule scope and strategy normalization

Each `ScheduleCertificate` identifies exactly one scheduling scope:

```text
scalar_control
scalar_region(region_id)
vector_epoch(epoch_id)
```

Vector epoch schedules are separate artifacts. They are never embedded in
`VectorPlanCertificate`, because changing `-ss` must leave both the plan hash and
the complete canonical plan-certificate bytes unchanged.

The schema stores the semantic enum, not the raw CLI integer:

```text
0   -> depth_first
1   -> breadth_first
2   -> special
3+  -> reverse_breadth_first
```

The original CLI token may appear in compiler diagnostics but not in canonical
cache identity or certificates. Thus `-ss 3` and `-ss 42` have the same
canonical strategy.

### 4.7 Type and effect normalization

Signal types use closed tagged objects:

```json
{ "kind": "int" }
{ "kind": "real" }
{ "kind": "tuple", "components": [{ "kind": "real" }] }
```

This v1 vocabulary intentionally matches the current Lean abstraction. FIR
widths and `FaustFloat` specialization belong in `RoutedFirCertificate`, where
the `fir_type` enum distinguishes `int32`, `int64`, `float32`, `float64`, and
`faust_float`.

Effects remain in semantic source order and use tagged resource objects. Unknown
or impure foreign calls are explicit; absence of an effect entry means a purity
claim that the verifier must justify from signal analysis.

Resource identities are derived from the prepared forest alone and are
strategy-independent (design in the port plan, section 4.4): state resources
use the owning signal id (hash-consing makes identical subtrees one signal,
hence one state — semantically exact); recursion-group state uses
`(group, projection index)`; tables use the creation-node id; UI zones use the
canonical UI path id; outputs use the channel index; foreign calls use the
declared name and signature with no purity inference. Verifiers recompute
every identity from the exported forest facts; an identity minted during
lowering or scheduling is a rejection.

## 5. Rust/Lean Mapping Contract

| Canonical definition | Lean definition | Planned Rust owner |
|---|---|---|
| `GraphSnapshot` | `DependencyGraph` | `crates/transform` generic scheduler module |
| `ScheduleCertificate` | `ScheduleCertificate` | `crates/transform` |
| `Strategy` | `SchedulingStrategy` | `crates/transform`, threaded by `crates/compiler` |
| `SignalDecorationRecord` | `Decoration` plus analysis facts | `signal_prepare` and `signal_fir::vector_analysis` |
| `DecorationCertificate` | representative `HasType` rules plus consistency obligations | `signal_fir::vector_verify` |
| `VectorPlan` | `VectorPlan` | `signal_fir::vector_analysis` |
| `VectorPlanCertificate` | `VectorPlanCertificate` | `signal_fir::vector_verify` |
| `Transport` | `Transport` | signal FIR vector routing |
| `RoutedFirCertificate` | `LoweringWitness` refinements | signal FIR router/verifier |
| execution equality | `VSimulation` | interpreter and backend differential gates |

The Rust types may be idiomatic and need not mirror Lean memory layouts. The
canonical DTO layer is the compatibility boundary. Conversion into a DTO must
be pure, deterministic, and tested independently from JSON rendering.

## 6. Progressive Integration Plan

### R0 - Freeze the minimal DTO shape

Deliverables:

- commit the v1 JSON Schema and this plan;
- add one valid and one invalid artifact example for each certificate kind and
  each schedule scope;
- validate examples structurally with a JSON Schema validator;
- pin the Lean version used in CI or document the elan toolchain requirement.

Pass criteria:

- Rust parses the valid minimal examples and rejects unknown fields/enums;
- DTO conversion is deterministic in one process and independent of `HashMap`
  iteration;
- the schema and provisional Rust checker are sufficient for the early vertical
  slice below.

Full RFC 8785 conformance, Lean-side SHA-256, cross-platform byte identity, and
the exhaustive invalid corpus are explicitly deferred until after RV. The
schema shape is useful before then; perfect canonical hashing is not allowed to
block the first semantic execution result.

### RV - Early executable vertical slice

Drive one nontrivial DSP through:

```text
prepared SigId forest
    -> provisional DecorationCertificate
    -> minimal signal-level VectorPlan
    -> routed FIR
    -> one executable backend
    -> scalar/vector output and final-state comparison
```

Required case properties:

- one shared sample expression and one pure tail;
- at least two loop regions and one typed cross-loop transport;
- one persistent state value compared after multiple blocks;
- bit-exact scalar, `-vec -lv 0`, and `-vec -lv 1` execution;
- a topology assertion that fails if the implementation silently serializes or
  inlines the entire case.

Use `-ss 0` and the Rust schedule postcondition checker initially. Do not wait
for four-strategy conformance, the Lean JSON importer, cross-OS hashes, all
backends, recursion groups, or clock islands.

Pass criterion: the signal-level planning path executes a genuine vector split
bit-exactly through one backend before broader structural assurance work grows.

### R1 - Schedule certificate at L2

Implement the generic Rust `GraphSnapshot`, `ScheduleCertificate`, and
`verify_schedule` before activating generalized `-ss`.

Required checks:

- canonical node and edge ordering;
- unique nodes and edges;
- every edge endpoint belongs to the node set;
- `node_count` agrees with both graph and order;
- `ordered_nodes` is a duplicate-free permutation;
- every dependency precedes its consumer;
- graph hash recomputation succeeds;
- all four strategies return typed cycle/malformed-graph errors.

The scheduler may be optimized later without changing the checker.

### R2 - Lean schedule importer and L3 CI

Add a Lean executable that:

1. reads a scoped `schedule_certificate` JSON artifact;
2. maps it to the existing Lean graph and strategy types;
3. recomputes the graph hash;
4. runs `verifySchedule`;
5. emits a `verification_result` artifact;
6. exits nonzero on malformed or rejected input.

CI must compare Rust and Lean acceptance on:

- the chain, diamond, asymmetric fork/join, disconnected, and path-heavy DAGs;
- exhaustive upper-triangular DAGs through six nodes;
- relabelled graphs and randomized insertion orders;
- deliberately corrupted node counts, orders, edges, and hashes.

Complete the deferred canonical boundary here:

- add shared RFC 8785 and SHA-256 test vectors;
- require Rust and Lean to compute identical canonical subject hashes;
- prove snapshot byte/hash identity on Linux, macOS, and Windows;
- reject duplicate object keys before canonicalization.

RV must already be green before this work becomes a phase blocker.

### R2D - Decoration certificate at L2/L3

Export `DecorationCertificate` directly from the real prepared `SigId` forest.
Each signal record contains:

- stable signal id and canonical signal type;
- variability and vectorability;
- clock-domain id;
- conservative ordered effects;
- context-sensitive occurrence and execution-condition facts;
- delay-read shape and maximum delayed use;
- labelled dependencies with stable endpoints.

The Rust and Lean checkers verify exact reachable-signal coverage, consistency
with the total type/clock maps, dependency endpoints, and effect coverage. The
Lean `Expr`/`HasType` subset remains explanatory; no compiler pass translates
production signals into that mini-AST merely to satisfy the checker.

The certificate is the exported projection of the in-memory `SignalUseInfo`
table (port plan, section 4.3); both must be produced by the same unified
traversal and the same `signal_dependencies` child walk, so the exported facts
cannot drift from the facts the planner actually consumed.

**Status (2026-07-13, P4.2/P4.3a).** P4.2 now provides the single typed decoder,
distinct scheduling and occurrence projections, and a deterministic in-memory
`SignalUseTable` over verified prepared signals. Its specialized delay,
projection, FIR/IIR, table, wrapper, recursive-variability, and sharing rules
are source-pinned to the C++ implementation, with explicit Rust representation
adaptations. At the P4.2 checkpoint this was only an R2D input: effect atoms,
the production execution-condition map, canonical export, and independent
coverage/consistency verification were still missing. The stable
signal-by-signal C++ occurrence oracle remains open after P4.3b.

**Status (2026-07-13, P4.3b).** Rust now has the real in-memory
`DecorationCertificate` projection and an independent acceptance boundary.
The DTO retains exact prepared `SigId` coverage, exact type fields rather than
the deliberately loose `SigType` inference equality, the complete condition
table and context-sensitive occurrences, compute effects, delay/projection
facts, and both labelled dependency views. `verify_decorations` recomputes the
canonical analysis, consults the authoritative prepared type and clock maps,
checks canonical order and exact endpoints/facts, and returns an opaque
`VerifiedDecorationCertificate`. Mutation tests reject every required class of
tampering before vector planning.

The accepted scope is deliberately `Compute`; visible `Gen` nodes are recorded
as lifecycle boundaries and `FullLifecycle` certificates are rejected until
initializer effects are decorated. JSON canonicalization, cross-language
hashing, and the corresponding executable Lean checker remain R2/RV work, so
this status does not claim the complete L2/L3 cross-language gate.

Pass criterion: mutating any signal type, clock, effect, delay, occurrence, or
dependency endpoint makes at least one independent verifier reject the
certificate before `VectorPlan` construction.

### R3 - Vector plan certificate at L2/L3

Implement the strategy-independent `VectorPlan` DTO and verifier.

Required checks:

- `decoration_hash` names an independently accepted certificate for the same
  program and semantic options;
- unique signal, loop, epoch, transport, and stable-name identities;
- exact epoch coverage and unique epoch ranks;
- ownership/root equivalence and inline duplicability;
- complete edge endpoints and no loop self-edge after normalization;
- acyclic induced graph for every epoch;
- complete typed transports for every cross-loop current-sample read;
- effect ordering or proven commutation for incomparable loops;
- monotone cross-epoch barriers;
- recursion groups and clock islands remain serial;
- every vectorizable loop has a recognized `VecSafe` witness kind;
- changing `-ss` leaves canonical plan bytes and `plan_hash` unchanged.

Semantic safety and vectorization retention are separate gates. For every
versioned case known to vectorize, record minimum vectorizable roots/loops,
required transports, and allowed named fallback reasons. Reject an unexplained
all-serial plan even if scalar/vector execution remains bit-exact.

The Lean checker should initially mirror these finite checks. Deeper semantic
witnesses can replace enumerated witness tags as the execution model matures.

Implementation status (2026-07-13, P4.4): the Rust `VectorPlan` DTO,
`verify_vector_plan`, production construction from an opaque
`VerifiedDecorationCertificate`, real PV-plan projection, and
strategy-dependent per-epoch execution scheduling are implemented as additive
L2 candidates. Construction derives placement, typed transports, effect
ordering, serial/vectorizable loop kinds, and witnesses without accepting
`-ss`, then returns only an opaque `VerifiedVectorPlan` after independent
verification. Scheduling verifies the plan before use, keeps fixed epoch-rank
order, applies the common four-strategy scheduler only inside each epoch, and
runs the independent postcondition checker. The verifier now derives effect
duplicability and local `VecSafe`, checks redundant loop/epoch consistency and
canonical witnesses, and requires every conflicting loop-effect pair to be
ordered. The `decoration_hash`, canonical serialization/hash, complete
cross-language Lean R3 checker, routed-FIR transport proof, and
vectorization-retention corpus remain open; this status does not claim R3
completion.

### R4 - Routed FIR certificate

After signal-level routing, emit and verify:

- one region for every materialized loop and fixed control domain;
- signal-to-FIR value types;
- producer stores and consumer loads for each transport;
- identical chunk-local index expressions on both sides;
- exactly-once emission of nonduplicable effects;
- fixed epoch-body order and selected intra-epoch schedules;
- no undeclared cross-region value reference;
- no strategy-dependent storage name or allocation.

Backend generation is forbidden unless this certificate is accepted.

Implementation status (2026-07-14, partial R4/P5.3): Rust now has additive
`VectorRouteSession` and independent `verify_routed_fir` gate. The route starts
only from `VerifiedVectorPlan`, materializes scheduled loop regions and the
three visibility scopes, and emits the plan's canonical typed transport
declarations, producer stores, and consumer loads at the exact `i0 - vindex`
index. Verification reconnects each store to the declared producer value and
each load to a recorded consumer use, checks value types and region visibility,
and rejects missing or mutated evidence. This establishes the structural
transport and value-routing subset of R4. Exactly-once effect traces, complete
epoch-body order, per-region CSE, actual signal-expression routing, Lean-side
checking, and the backend activation gate remain open; therefore no full R4
certificate is claimed yet.

The P5.2 pure executable slice now lowers actual verified prepared signal
closures into those regions and runs CSE independently in each control/loop
scope before route values are sealed. A second independent postcondition
checker proves that every accepted P5.1 definition and transport store/load is
present in the final CSE-rewritten body assigned to its region. Tests cover
both FIR real precisions, all four schedules, a rejecting body mutation, and
fail-closed stateful input. This advances the value/transport/body subset of R4,
but it does not by itself establish effect traces, state transitions, full
epoch order, Lean-side acceptance, or backend eligibility.

P5.3 adds a bounded producer/checker pair over the exact P4.4 plan retained by
the routed artifact. It expands the complete chunk into canonical definition,
use, transport, effect, and epoch-boundary events; derives a sample-major
scalar order, the routed loop-major vector order, and a conservative dynamic
dependence relation; then exhaustively checks the Lean-shaped `FissionSafe`
predicate. Conflicting state or observable effects in separate loops expose
cross-sample reversals and are rejected even when a static effect edge orders
the loop DAG. Co-located serial state and pure transported data are accepted.
The explicit bound fails closed instead of checking a prefix. This is concrete
Rust evidence for the finite R4 ordering obligation, but the Rust certificate
is not yet serialized or accepted by Lean.

P6.1 now adds a second finite producer/checker artifact for delay and recursion
state. It records the C++ copy/ring storage geometry, canonical loop
`pre/exec/post` words, simultaneous recursion projection groups, and exact
managed state resources. Bounded concrete copy/ring executions are compared
through `alpha` with `history_step`/`delay_read`, and accepted transition words
refine P5.3 effects into phase events with explicit barriers and recursion-step
chains. This advances the delay/recursion subset of R4/R5, but it is not yet
serialized or accepted by Lean, and it does not cover tuple FIR routing, final
module assembly, or backend eligibility.

P6.2 adds a third finite producer/checker artifact for clocks and AD policy. It
recomputes `ClkEnvMap`, checks the domain tree and wrapper-kind agreement,
reconstructs one nested serial island per OD/US/DS wrapper, and partitions
transports into top-rate outer-chunk, island-scalar, and persistent held-output
routes. Its executable `ClockStep`
model covers boolean, counted, and modulo firing while preserving state and
held output on zero fires. FAD is accepted only in its post-propagation ordinary
signal form. Reverse carriers are not vector-certified: they produce a stable
scalar-fallback diagnostic and immutable `Forward < Reverse` epoch evidence.
This advances the clock/AD-policy subset of R4/R5, but the artifact is not yet
serialized or checked by Lean and does not establish final FIR simulation or
backend eligibility.

P6.3a removes the tuple-definition gap at the routed-FIR boundary without
inventing a backend tuple ABI. Recursive tuple values use a deterministic
structural FIR type and a recursively checked component constructor; the
checker rejects incorrect arity or nested component types even when the outer
type annotation is forged. Matching C++ `sigProj` lowering, inter-loop tuple
transports remain invalid and scalar projections carry the values. The real
two-projection recursion fixture closes routing under every `-ss` strategy.
This advances the tuple/projection subset of R4, but stateful projection
lowering, phase/island FIR assembly, serialization, Lean acceptance, and
backend eligibility remain P6.3b or later work.

P6.3b adds a fourth finite producer/checker artifact at the final-region
boundary. The producer composes accepted P5 routes with P6.1 phase words and
P6.2 island/transport policy; the checker independently requires exact loop,
phase-action, island-parent, guard-kind, and transport-lifetime coverage. Copy
and ring storage use the C++ index equations, recursive projections are
captured before any projection write, island scalars are declared below their
guard, and held outputs are persistent cleared state. Reverse-AD fallbacks fail
closed before assembly. This advances the concrete FIR side of R4, but the
artifact is not yet canonical JSON, hash-bound to its producers, checked by
Lean, placed into a final module, or admitted to backend execution.

P6.4 composes the accepted finite artifacts into a complete module for the
pure supported subset. The final checker validates lifecycle placement,
accepted-body inclusion, and exact output-store coverage, then requires the
generic FIR scope/type checker to report no errors. Production selection runs
only after P5.3, P6.1, P6.2, and P6.3b acceptance. Unsupported shapes retain a
stable, observable fallback reason; that preserves behavior but is explicitly
not evidence that the fallback module satisfies the higher assurance level.
The final module artifact is still in-memory Rust FIR: canonical
serialization, hash binding, Lean acceptance, and stateful/clocked differential
execution remain open.

P6.5 extends production execution without raising the assurance claim. The
generalized lowerer may use a state effect only when it is named by the
accepted P6.1 plan or delegated to the accepted P6.2 clock/hold plan. Fixed
delay reads instantiate the certified copy/ring equations, symbolic recursion
resolves through its binder, recursion next-value declarations remain visible
until simultaneous writes complete, and a held clock output is stored after
its guard on every outer sample. The assembly schema is versioned so these
scope and placement rules are checked as part of the finite artifact.

Bounded scalar/vector differential execution now covers fixed delays,
top-rate recursion, stateless clock islands, and expanded FAD under both loop
variants and all scheduling strategies. This is translation-validation
evidence, not a Lean proof. Variable delays, clock-local state, UI, and reverse
AD are named fallbacks. Canonical serialization, producer hashes, Lean R3/R4
acceptance, and C++ corpus retention are still required before the certified
porting claim can be strengthened.

P6.6 extends the finite state and assembly artifacts without claiming an
unbounded semantic proof. State schema v2 records either top-rate copy/ring
storage or a clock ring `(domain, buffer, shared cursor, capacity, mask)`.
Assembly schema v3 records the optional cursor advance inside each guard. The
Rust checker independently recomputes domain-loop ownership from the accepted
clock plan and requires `capacity = pow2ceil(maxDelay + 1)`, masked reads and
writes, one common cursor name per domain, and exactly one `cursor := cursor+1`
per fire. A negative mutation removes that word and must be rejected.

Bounded variable delays instantiate the same checked storage equation with a
runtime amount whose type-derived upper bound fixes the allocation. Delayed
cross-loop dependencies add an ordering edge but no immediate transport, so
the chunk producer precedes its readers unless that edge would close a checked
feedback path. Scalar/vector traces now cover these rules for `lv=0/1` and
`ss=0/1/2/3`. The remaining proof obligation is still
R5: relate these finite equations and guarded event traces to an executable
signal semantics, then bind the accepted Rust artifacts to canonical R3/R4
serialization and Lean checking.

### R5 - Semantic reference execution

Extend the Lean model with a small executable signal/FIR semantics in increments:

1. constants, inputs, and pure arithmetic;
2. casts and tuple projections;
3. bounded delays;
4. recursion groups;
5. tables, UI, and effect observations;
6. clock-domain islands;
7. forward and reverse AD epochs.

For each increment, generate bounded programs and inputs, then compare:

```text
Lean scalar reference
Rust scalar interpreter
Rust vector interpreter, lv=0 and lv=1, ss=0/1/2/3
C++ reference where the behavior is implemented there
```

Use bit equality at current impulse-test boundaries. Any intentionally relaxed
numeric relation must be stated per operation and must not be introduced by
scheduling.

### R6 - Selected L4 refinement

Attempt deductive connection only after DTOs and executable checkers stabilize.
Prioritize pure, ownership-free functions:

- strategy decoding;
- schedule verification;
- loop-separation precedence;
- canonical sorting and projection construction;
- transport index bounds;
- epoch-barrier validation.

Possible implementation-proof approaches include translating a restricted Rust
subset into Lean or proving an equivalent functional model and validating the
compiled Rust through generated conformance tests. Tool choice is deferred until
a prototype demonstrates support for the repository's Rust edition, enums,
collections, error model, and CI platforms. No tool is accepted merely because
it can parse a toy function.

### R7 - Backend refinement gates

Treat backend correctness as refinement of certified FIR, not as a repetition of
the signal analysis proof.

For representative FIR programs require:

```text
Execute(Cranelift(FIR))      = Interpret(FIR)
Execute(Wasm(FIR))           = Interpret(FIR)
Execute(AssemblyScript(FIR)) = Interpret(FIR)
```

Initially this is differential testing at L1/L3. Formal backend proofs are a
separate long-term project because they include ABI, runtime, and target-machine
semantics.

## 7. CI and Developer Workflow

### Fast gate on every change

```text
cargo test for touched checker crates
canonical schema/example validation
Rust certificate negative tests
Lean compilation of the formal specification
small Rust/Lean cross-check corpus
```

### Workspace gate before merge

```text
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo run -p xtask -- golden-check
complete versioned certificate corpus through Lean
```

### Scheduled exhaustive gate

- exhaustive small DAG and small plan enumeration;
- randomized larger graphs with deterministic seeds;
- scalar/vector/backend differential execution;
- C++ topology and runtime comparison;
- canonical snapshot reproducibility on all CI operating systems.

Artifacts emitted by CI must use repository-relative case ids and record the
Rust and Lean verifier versions. A verification result never overrides a hash
mismatch or schema failure.

## 8. Negative Testing Policy

Every semantic check needs a mutation that proves rejection. Minimum mutations:

- remove, duplicate, or reorder one graph node;
- reverse one dependency edge;
- place a consumer before its dependency;
- alter `graph_hash` or `plan_hash`;
- alter one decoration type, clock, effect, occurrence, delay fact, or labelled
  dependency endpoint;
- duplicate a loop across epochs;
- change an epoch rank;
- assign an owned root to the wrong loop;
- mark a nonduplicable signal inline;
- remove one required transport;
- alter a transport type or length;
- remove an effect edge between conflicting loops;
- move a loop between forward/reverse AD epochs;
- change one routed FIR store/load index;
- duplicate or omit an effectful FIR statement.
- replace every vectorizable loop with an unexplained serial fallback;
- (once lockstep bundles exist) corrupt one lane of an isomorphism witness or
  bundle two loops connected by a dependency path.

A checker without a demonstrated rejecting mutation is not complete enough to
serve as a trust boundary.

## 9. Schema Evolution and Compatibility

- v1 is internal and experimental until R2 exits;
- once used by committed CI artifacts, incompatible changes require v2;
- the Lean and Rust readers declare the exact version set they accept;
- a verifier must never silently drop unknown fields;
- old artifacts remain verifiable with their pinned verifier or an audited
  explicit converter;
- semantic changes require a plan/journal entry even when JSON shape is
  unchanged;
- hash projection changes always require a schema version change;
- the planned lockstep-bundling extension (see the port plan's "Lockstep
  instance vectorization extension" section) is a v2 candidate: it adds a
  `lockstep` loop kind with a `width`, an isomorphism witness carrying one leaf
  mapping per lane, and a `layout` field (`planar`/`interleaved`) on transports.
  It must not be retrofitted into v1 objects.

## 10. Completion Criteria

The certified porting architecture is operational when:

1. the RV signal-level vertical slice is bit-exact and demonstrably vectorized;
2. the canonical schema has shared Rust/Lean parser and hash vectors;
3. decoration and scheduler results are L2 checked before vector planning;
4. vector plans and routed FIR are rejected before emission on any failed check;
5. CI rechecks all versioned artifacts with Lean;
6. `-ss` changes only certified schedules, never `VectorPlan` identity;
7. `-vec -lv 0` and `-vec -lv 1` match scalar execution across all supported
   backends and scheduling strategies;
8. the vectorization-retention corpus has no unexplained all-serial fallback or
   coverage decrease;
9. every unsupported semantic case fails with a typed diagnostic rather than
   bypassing certification;
10. the remaining trusted computing base and unproved obligations are listed in
   the current handoff and release documentation.

This architecture does not turn the complete compiler into one monolithic Lean
proof. It creates a sequence of small, auditable proof and verification
boundaries so that the Rust port can progress without postponing assurance until
the implementation is finished.
