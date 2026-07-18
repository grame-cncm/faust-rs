# Lockstep SIMD Remediation Plan

Date: 2026-07-16

Status: complete; Steps 0-4 implemented on 2026-07-16

Scope: checked signal-level vector mode, lockstep event certification, FIR state lowering, and C++ SIMD evidence

## 1. Decision

Keep the existing lockstep legality analysis, bundle certificate, scheduler
unit, and planar external ABI. Improve the implementation at two lower
boundaries:

1. represent repeated lockstep event-order evidence compactly so the default
   `-vs 32` path does not fall back merely because a finite checker table
   crosses its implementation-size limit;
2. lower delay-one lockstep recursion as register-carried lane state instead of
   four independent chunk-history arrays, preserving the scalar IEEE operation
   sequence while exposing a profitable SLP shape to C/C++ optimizers.

The native SIMD gate must first prove that checked vector FIR was retained. A
`-vec` request followed by scalar fallback is not lockstep evidence, even if the
fallback C++ happens to be SLP-vectorized.

## 2. Audit result and corrected baseline

The original `lockstep-simd-check` introduced by commit `111198ba` compiled the
requested vector mode directly to C++ and inspected optimized LLVM IR. It did
not inspect `FirCompileOutput::vector_pipeline_status` before code generation.
That omission allowed scalar fallback to satisfy the SIMD assertion.

For `tests/corpus/vector_lockstep_simd_quad.dsp`, the observed baseline is:

| Request | Effective mode | Generated shape | Four-wide LLVM FP operations |
|---|---|---|---:|
| `-vec -lv 1` (default `-vs 32`) | scalar fallback: `EventCertificate` | byte-identical to scalar C++ | 14 |
| `-vec -vs 24 -lv 1` | checked vector FIR | chunk driver plus four `vstate_*_tmp` histories | 0 |

The default-size fallback reports:

```text
bounded event model needs 4243 events, limit is 4096
```

This does not reject the DSP semantics. The checker expands one complete vector
chunk into a finite table of definitions, uses, effects, transports, and state
transitions. The fixed 4096-entry implementation bound protects checker cost;
the four-lane arithmetic chain expanded over 32 samples requires 4243 entries.
Reducing the chunk to 24 samples stays below the bound and exposes the actual
checked-vector lowering.

Clang reports the certified `-vs 24` C++ as SLP-legal but not profitable:

```text
List vectorization was possible but not beneficial with cost 0 >= 0
```

The dominant structural difference is state storage. Scalar fallback carries
four recursion values in scalar locals that Clang packs. Checked vector FIR
loads and stores four separate arrays (`vstate_s*_tmp`) on every sample and
copies their chunk boundaries, which removes the profitable register-carried
shape.

## 3. Existing invariants that remain frozen

- Lockstep detection stays automatic under `-vec`; no new user option is added.
- Exact prepared-signal isomorphism, graph incomparability, effect commutation,
  common clock/epoch, and fail-closed rejection remain mandatory.
- Logical loop ids, recursion-group ids, and lane order remain stable.
- The external `compute` ABI remains planar.
- Each lane retains its scalar IEEE operation order. No algebraic
  reassociation, parallel-form IIR conversion, or fast-math dependency is
  introduced.
- FMA contraction policy must be identical between scalar and vector paths.
- Unsupported delay, control-flow, effect, or transport shapes retain their
  current checked-vector form or fall back safely.

## 4. Workstream A: compact event-order evidence

### 4.1 Problem

`vector/events.rs` currently materializes every event template once per sample
for the complete `vec_size`. This is useful as a small independent witness but
scales linearly with chunk size and lane-body complexity. Raising 4096 to 8192
would unblock the current example but would only move the next fallback and may
increase pairwise dependency-checking cost substantially.

### 4.2 Proposed representation

Add a compact repeated-region witness for verified lockstep bundles. It should
record:

- the canonical per-sample event templates for each logical lane;
- the checked lane mapping and bundle width;
- the sample interval `0..vec_size`;
- intra-sample dependencies;
- loop-carried dependencies from sample `n` to `n + 1`;
- fixed `LoopPre` and `LoopPost` transitions.

The producer may use the existing expanded model as a development oracle on
small chunks. The production checker must independently validate the compact
witness without trusting producer-computed totals or dependency edges. It must
check template coverage, sample-range arithmetic, lane membership, carried
state adjacency, and the equivalence of scalar-major and lockstep-major orders
under the already-certified commutation obligations.

The compact representation must remain finite and explicitly bounded by the
number of templates, edges, and bundle descriptors. It must not claim a formal
proof beyond the executable certificate assurance level documented by the
vector plan.

### 4.3 Acceptance criteria

- `vector_lockstep_simd_quad.dsp` reports `VectorPipelineStatus::Certified` and
  `VectorEffectiveMode::CertifiedVector` at default `-vs 32`, for `-lv 0/1` and
  all four scheduling strategies.
- A mutation of the sample interval, lane mapping, carried edge, or event
  template is rejected by the independent checker.
- Small-chunk expanded and compact certificates accept and reject the same
  generated mutation corpus.
- Existing non-lockstep event certificates are unchanged unless a shared
  representation is proven equivalent by tests.
- Checker time and peak memory are measured before selecting descriptor bounds;
  no limit increase is accepted without recorded measurements.

## 5. Workstream B: register-carried lockstep state

### 5.1 First supported shape

Start with exact top-rate lockstep bundles whose lane recursion has one scalar
projection and maximum delay one. This covers the committed SIMD-oriented
corpus while keeping the state transformation explicit and independently
checkable.

For each lane, load the persistent prior state once before the physical sample
loop, carry it in a local scalar, and store it once after the loop. The emitted
C++ should have the following semantic shape:

```cpp
float s0 = persistent_state0;
float s1 = persistent_state1;
float s2 = persistent_state2;
float s3 = persistent_state3;

for (int i = begin; i < end; ++i) {
    float y0 = lane_body(input0[i], s0);
    float y1 = lane_body(input1[i], s1);
    float y2 = lane_body(input2[i], s2);
    float y3 = lane_body(input3[i], s3);
    output0[i] = y0;
    output1[i] = y1;
    output2[i] = y2;
    output3[i] = y3;
    s0 = y0;
    s1 = y1;
    s2 = y2;
    s3 = y3;
}

persistent_state0 = s0;
persistent_state1 = s1;
persistent_state2 = s2;
persistent_state3 = s3;
```

The FIR representation should co-locate the bundle id, ordered lane state
identities, initialization sources, and final stores in one checked record.
Assembly must not infer register eligibility from generated variable names.

### 5.2 State-plan and checker obligations

- exactly one persistent load and one persistent store exist per lane;
- the local state value used at sample `n` is the value produced at sample
  `n - 1`, or the persistent entry value for the first sample;
- lane states are neither crossed nor duplicated;
- every early exit or guarded clock path is excluded initially unless the
  checker proves its final-store behavior;
- chunk boundaries preserve the same state as scalar execution;
- output stores and state updates retain canonical lane order without changing
  each lane's expression tree.

The current `vstate_*_tmp` path remains the fallback for unsupported state
shapes. Register carrying is an internal FIR/state representation adaptation,
not an ABI change.

### 5.3 Longer delays

After delay-one acceptance, extend only when justified by corpus evidence.
Prefer a structure-of-arrays layout indexed as `state[delay][lane]`, so values
for one delay across all lanes are contiguous. Do not silently transpose the
external audio ABI. Any chunk-local input/output interleaving must have an
explicit transport-layout record and measured benefit.

### 5.4 Acceptance criteria

- Certified vector C++ for the delay-one SIMD corpus no longer declares or
  updates per-lane `vstate_*_tmp` arrays inside the lockstep kernel.
- Clang `-O3 -ffp-contract=off -S -emit-llvm` emits at least ten arithmetic
  operations on `<4 x float>` for the certified lockstep region.
- The same gate passes for the mixed reduction and unrelated-side-branch
  corpus, and source/IR attribution confirms that SIMD belongs to the lockstep
  region rather than a separate time-vectorizable loop.
- Scalar and vector interpreter outputs are bit-identical for tail chunks,
  both loop variants, all scheduling strategies, and optimization levels zero
  and maximum.
- A malformed lane-state mapping and a missing final persistent store are
  rejected by structural mutation tests.

## 6. Workstream C: trustworthy native-SIMD gate

Replace the current direct C++ compile in `lockstep-simd-check` with this order:

1. compile the source to `FirCompileOutput` using the requested vector mode;
2. require `VectorPipelineStatus::Certified`;
3. require `VectorEffectiveMode::CertifiedVector` and no fallback detail;
4. verify the expected checked-vector chunk/lockstep FIR structure;
5. generate C++ from that exact verified FIR module;
6. compile the wrapper with Clang `-O3 -ffp-contract=off -S -emit-llvm`;
7. count vector floating-point operations attributable to the lockstep kernel.

The gate must fail on the current baseline. A scalar fallback, scalar-mode
compilation, missing bundle, vector instructions from an unrelated loop, or a
forced-vectorizer option must all be negative tests.

Do not use target-specific assembly mnemonics as the primary assertion.
Optimized LLVM vector IR is the portable gate; optional AArch64/x86 assembly
inspection may remain diagnostic evidence.

## 7. Implementation sequence and commits

### Step 0 — correct the trust boundary

- Make the SIMD gate require effective checked-vector status.
- Add the scalar-fallback false-positive regression.
- Mark the previous 14/17/14 result as invalid lockstep evidence in the plan and
  journal.

Pass condition: the corrected gate fails for the documented current reason and
cannot pass on scalar fallback.

Implementation status (2026-07-16): complete. The gate now lowers to
`FirCompileOutput`, requires both certified status and effective checked-vector
mode with no fallback detail, and only then emits C++. Unit tests reject an
`EventCertificate` scalar fallback and accept the exact certified tuple. The
command fails on the current default-size corpus with the expected 4243/4096
diagnostic, establishing the intended red baseline for Step 1.

### Step 1 — compact lockstep event certificate

- Add the compact schema, producer, independent checker, and mutations.
- Retain expanded/compact differential checking on bounded fixtures.

Pass condition: all three complex cases retain checked vector FIR at default
`-vs 32` under the complete strategy/loop-variant matrix.

Implementation status (2026-07-16): complete. `EventOrderCertificate` now
binds the logical chunk length separately from its concretely checked sample
count. Plans with a verified lockstep bundle retain complete expansion while it
fits; otherwise they use the canonical two-sample basis. The first sample
checks every static event template and the second checks adjacent carried
state. The independent checker reconstructs the basis size from routed FIR,
checks translation-identical per-loop templates, and rejects basis, template,
and recursion-edge mutations. All three complex cases are certified at default
`-vs 32` across both loop variants and all scheduling strategies. The corrected
SIMD gate now reaches native code and reports zero vector operations, which is
the intended red baseline for Step 2.

### Step 2 — register-carry delay-one state

- Add the state-plan record, FIR assembly, and independent structural checks.
- Keep unsupported shapes on the existing array-backed path.

Pass condition: bit-exact tests pass and the corrected native-SIMD gate observes
profitable four-wide arithmetic in the certified region.

Implementation status (2026-07-16): complete. State-plan schema v4 introduces
an explicit `LockstepRegisterBundle` whose ordered lane records bind bundle,
loop, recursion group, delayed carrier, local scalar, and persistent scalar
identities. The producer promotes only complete top-rate bundles with one
recursion projection and one fixed delay-one carrier per lane; the independent
checker reconstructs that mapping, and unsupported or partial shapes retain
the existing copy/ring storage. FIR loads each persistent value once per chunk,
carries it in a scalar local, and stores it once after the physical loop.
Lockstep assembly transposes isomorphic statement rows across lanes, exposing
homologous operations to SLP without changing any lane's expression order.
Mutation tests reject crossed lanes and a missing final store. The three
certified default-size corpus cases produce respectively 14, 30, and 22
four-wide LLVM floating-point operations with Clang 14 at
`-O3 -ffp-contract=off`; scalar/vector interpreter results remain bit-exact
across both loop variants, all four schedulers, and tail chunks.

### Step 3 — mixed-subgraph validation

- Prove that the reduction and side branch remain outside bundle membership.
- Attribute LLVM SIMD to the bundle's generated source range or stable helper.

Pass condition: both mixed corpus cases remain partially lockstep, produce
certified SIMD in that region, and retain their separate non-bundle behavior.

Implementation status (2026-07-16): complete. The native gate locates the one
generated physical sample loop containing checked `vlock_*` state and requires
the mixed cases to retain their second physical sample loop. Clang now emits
line-table provenance, and only four-wide floating-point instructions whose
`DILocation` lies inside that lockstep source range count. Focused negative
tests reject an incorrect physical-loop count and demonstrate that SIMD tagged
to an unrelated source range is excluded. The principal, mixed-reduction, and
mixed-branch cases each have 14 lockstep-attributed operations; their complete
module totals are 14, 30, and 22 respectively.

### Step 4 — repository gates

- Run formatting, workspace Clippy, focused mutation/unit/integration tests,
  workspace tests, `vector-interp-opt-check`, golden checks, and the maintained
  vector impulse matrix.
- Update Rustdoc source provenance, the section-8 plan, daily journal, and
  session handoff in every implementation commit.

Pass condition: no lockstep fallback or parity regression is introduced. Any
unrelated existing workspace failure is reported separately and must not be
represented as a passing gate.

Implementation status (2026-07-16): complete. Formatting and workspace Clippy
pass with warnings denied. All 367 transform tests, 27 vector-mode integration
tests, 35 xtask tests, the 40-trace optimized/unoptimized interpreter matrix,
the tracked Rust golden corpus, and the attributed native-SIMD gate pass. After
an explicit release rebuild of `impulse-runner`, the forced maintained impulse
matrix compiles and compares 92 expected DSPs under each of `-lv 0` and
`-lv 1`; `subcontainer1` remains the documented exclusion. The workspace test
run exposed and fixed the stale Wasm pure-`Drop` expectation; all 285 active
codegen tests then pass. The complete workspace run proceeds to an unrelated
existing `p3_shadow_mode` assertion: recursive APF produces three distinct C++
scheduling forms where that test expects four. This scalar scheduling issue is
reported separately and is not represented as a passing workspace gate.

## 8. Risks and mitigations

- **Unsound certificate compression:** cross-check compact and expanded models
  on small cases and require independent mutation rejection.
- **Hidden reassociation or contraction drift:** preserve lane expression trees,
  compile evidence without fast-math, and compare exact interpreter bits.
- **C++ alias-cost instability:** retain planar ABI and use register-carried
  states first; introduce layout-changing transports only with explicit records
  and measurements.
- **Backend-specific leakage:** keep the FIR state contract backend-neutral;
  Clang LLVM inspection validates one backend optimization, not language-level
  semantics for every backend.
- **Over-broad eligibility:** begin with delay-one, top-rate, unguarded scalar
  recursion and fail closed for every other shape.
- **Checker resource growth:** bound compact descriptors and benchmark the
  checker instead of repeatedly raising the expanded-event ceiling.

## 9. Required validation commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p transform --lib
cargo test -p compiler --test vector_mode
cargo test -p xtask --all-targets
cargo run -p xtask -- vector-interp-opt-check
cargo run -p xtask -- lockstep-simd-check
cargo run -p xtask -- golden-check
make -j8 interp-vec0 interp-vec1 -C tests/impulse-tests
cargo test --workspace --all-targets
```

Every reported SIMD result must include the requested vector size, effective
pipeline status, compiler and optimization flags, vector-operation count, and
the corpus case whose lockstep region produced those operations.
