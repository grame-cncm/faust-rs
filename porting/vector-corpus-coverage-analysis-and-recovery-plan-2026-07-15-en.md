# Vector Corpus Coverage Analysis and Recovery Plan

Date: 2026-07-15

Status: complete — Phases 0 through 6 complete

Working branch: `ondemand-vec-fad-synthesis`

Related documents:

- [`vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`](vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md)
- [`vector-mode-fir-model-hardening-plan-2026-07-10-en.md`](vector-mode-fir-model-hardening-plan-2026-07-10-en.md)
- [`vector-fused-recursive-delay-plan-2026-07-15-en.md`](vector-fused-recursive-delay-plan-2026-07-15-en.md)
- [`lean-rust-certified-porting-plan-2026-07-11-en.md`](lean-rust-certified-porting-plan-2026-07-11-en.md)

## 1. Executive Summary

Only 3 of the 93 DSP files in `tests/impulse-tests/dsp` currently complete the
checked signal-level vector pipeline for `--double -vec -lv 0 -ss 0`. This low
number does not mean that the earlier `LoopGraph` implementation lost the
ability to construct vector-shaped code for almost the entire corpus. It is
mainly the result of a deliberate policy change in commit `b5a0a8b3`:

```text
before b5a0a8b3: checked-pipeline rejection -> transitional builder in Vector mode
after  b5a0a8b3: checked-pipeline rejection -> scalar builder
```

The policy change fixed a real correctness defect involving recursive delayed
reads transported across vector loops. It was correct to stop treating the
transitional path as generally safe. However, the scalar fallback was applied
to every checked-pipeline rejection, including UI programs and unrelated
unsupported forms. The 67 UI rejections therefore dominate the observed loss
of effective vector-mode coverage.

The recovery must not revert the global safety fix. It should move the useful
capabilities of the transitional builder into the checked pipeline, starting
with UI lifecycle integration, while retaining producer/checker evidence and
explicit scalar fallback for unsupported temporal dependencies.

### 1.1 Implementation Progress

Phase 0 was completed on 2026-07-15. `SignalFirOutput` and the public compiler
FIR result now retain the effective emitted mode and the complete first-failure
detail in addition to the stable `VectorPipelineStatus`. The corpus counter can
emit a machine-readable JSON report with per-file status, effective mode,
detail, errors, and aggregate fallback-reason counts.

The Phase 0 corpus run confirmed that every one of the 89 fallbacks emits
`Scalar` and produced the following downstream classification:

- `VectorPlan`: one delayed-recursive crossing, three table transports, three
  invalid root placements, and eight missing sample placements;
- `StatePlan`: one delay owned by a non-vector loop and three unsupported
  `WaveformIndex` resources;
- `PureLowering`: one foreign sampling-frequency constant and two routed
  consumer bodies missing their transport load;
- `UiProgram`: 67 programs still stopped by the first-stage UI guard.

The public API mapping is additive: existing status matching remains valid,
while `VectorEffectiveMode` and `vector_pipeline_detail` add observability with
no C or C++ ABI impact.

Phase 1 was completed on 2026-07-15. Scalar and vector FIR assembly now share
the canonical UI zone-naming and grouped `buildUserInterface` statement
builder. The checked vector path adds an explicit `VectorUiFir` artifact whose
declarations, reset statements, and grouped UI statements are compared exactly
at the final module boundary. Buttons, checkboxes, sliders, numerical entries,
bargraphs, nested groups, metadata, and multi-control programs therefore pass
through the checked pipeline without weakening the existing plan, state,
effect, or route checks. Soundfile lifecycle declarations are understood, but
sound data lowering remains fail-closed with a specific `UiProgram` diagnostic.

The default corpus moved from 3 to 11 certified DSPs. The newly certified files
are `UITester.dsp`, `bargraph.dsp`, `noise.dsp`, `noisemetadata.dsp`,
`norm1.dsp`, `norm2.dsp`, `panpot.dsp`, and `volume.dsp`. The post-UI waterfall
is 58 `VectorPlan`, 11 `StatePlan`, 11 `PureLowering`, one soundfile
`UiProgram`, and one independent SIGGEN error. All eight new files passed 96
fresh native C++ impulse comparisons: scalar `-ss 0..3` plus vector
`-lv 0/1 x -ss 0..3`, each against the 60,000-frame Faust C++ oracle.

Removing the early UI guard initially exposed superlinear behavior in the
checked analysis rather than an inherent UI cost. Profiling identified three
repeated-graph algorithms: whole-map effect fixed points, per-pair BFS effect
orientation/verification, and repeated topological membership scans. These
now use a changed-node worklist, deterministic Kahn ordering, compact
transitive-closure rows, precomputed loop effects, and exact compact conflict
summaries. Representative debug-build times fell from about 73.5 to 6.9
seconds for `bells.dsp`, 153 to 10 seconds for `cubic_distortion.dsp`, and 75.7
to 65.8 seconds for `reverb_designer.dsp`; the last DSP already costs about
60.6 seconds in scalar mode. `count_vector_corpus --compare-scalar-time` now
reports that distinction directly.

Phase 2 was completed on 2026-07-15. The post-UI plan waterfall was first
partitioned into 20 delayed-recursion crossings, 18 unplaced sample nodes, 18
invalid non-sample/lifecycle owners, and two table-value transports. Placement
initially assigned owners to intrinsically sample-typed nodes, traversed the
complete checked dependency projection, and materialized disconnected effect
roots retained by decorations. Phase 3 subsequently refined that condition for
slow values identified as delay carriers by the occurrence certificate or as
state-dependent by checked effects. Stateful `waveform` table-typed nodes
cross loop boundaries as typed element-value transports rather than as copied
table identities.

`FusedSerialGroup` now supports multiple projections, arbitrary internal
transports, independent disjoint recursive slices, and transitively merged
components of coupled recursive owners. The independent checker reconstructs
every delayed read-to-writer edge, recursive owner, clock join, and internal
transport from accepted decorations; final assembly checks every delayed state
write inside the one physical per-sample loop. The complete 93-DSP sweep now
has zero `VectorPlan` fallback: 39 files first fail in `StatePlan`, 41 in
`PureLowering`, one soundfile remains in `UiProgram`, and `subcontainer1.dsp`
retains its independent SIGGEN error. The certified count intentionally stays
at 11 because Phase 2 changes only plan admissibility; no newly certified DSP
or generated sample path exists yet to claim as parity-tested.

Profiling caught a real debug-build regression before this phase was committed.
On `reverb_designer.dsp`, a vector request that later fell back took 80.3
seconds versus 59.3 seconds scalar. The causes were quadratic raw effect-set
comparisons in the independent plan checker and a second complete plan check in
the clock/AD producer. A checker-local compact effect summary, cross-checked
against atom-pair semantics, and reuse of the accepted `VerifiedVectorPlan`
reduced the vector request to about 65.2 seconds. Its remaining approximately
six seconds are attributable in stage traces to decoration and one plan build;
the 59-second frontend cost is unchanged and remains scheduled for the Phase 6
historical audit.

Phase 3 was completed on 2026-07-15. Its 39 incoming failures were 19 temporal
signals without an owner, 13 delays owned by conservative serial islands, six
waveform-index resources, and one hidden recursion projection. The key semantic
finding is that intrinsic Faust variability is not an execution-rate proof: a
constant carried by a cleared delay still changes after the first sample.
Placement now keeps certified delay carriers and state-dependent closures in
runtime loops, while pure slow dependencies, including fixed-delay amounts,
remain control values. Every top-rate
`Island` is accepted as a serial delay owner because final assembly already
emits it as an ordered per-sample loop.

The state artifact schema is now version 3. It explicitly represents cycling
waveform indexes, initialized `prefix` cells, conservative delay effects proven
to have zero history, and recursive projections that have an internal body but
no visible projection alias. Producer and checker both reconstruct these facts
from the verified prepared forest, decorations, and vector plan. Mutation tests
cover waveform geometry, and the FIR assembler independently checks the new
state-update statement shapes and lifecycle declarations. The internal Rust
API for `build_vector_state_plan_with_clock` is adapted to receive
`VerifiedPreparedSignals`; this has no CLI or C/C++ ABI impact and is required
to certify waveform length, prefix initialization, and hidden recursion bodies.

The complete counter now reports zero `StatePlan` and zero `VectorPlan`
fallback: 11 DSPs remain certified, 80 reach `PureLowering`, one soundfile
reaches `UiProgram`, and the independent SIGGEN error remains. This phase adds
state evidence but no complete new runtime path, so there is no newly certified
DSP or sample-parity claim yet. Those paths become executable in Phase 4.

Advancing the slow corpus cases exposed expected downstream work rather than a
regression in earlier stages. `reverb_designer.dsp` first rose from about 65.2
to 70.4 seconds because the newly accepted state plan cost 0.87 seconds and the
lowerer built a 2.70-second routing session before encountering an unsupported
sampling-frequency constant. A deterministic unsupported-node preflight now
fails before route construction, restoring the request to about 66.8 seconds.
Once Phase 4 supports that node, routing cost will be measured as certified
work and optimized rather than hidden by fallback.

Phase 4 started on 2026-07-15 with the sampling-frequency foreign constant.
Checked vector lowering now mirrors the scalar lifecycle contract by loading
the persistent `fSampleRate` field for `fSamplingFreq` and `fSamplingRate`,
with an explicit cast only when the verified FIR type is real. This moved the
default corpus from 11 to 23 certified DSPs. All 12 new paths passed the full
scalar `-ss 0..3` and vector `-lv 0/1 x -ss 0..3` native C++ impulse matrix.

This support removed the temporary foreign-constant preflight and made route
construction useful work. Profiling then exposed two redundant global plan
checks in the route-session setup. Downstream production scheduling now
accepts the opaque `VerifiedVectorPlan`, while public raw-plan APIs preserve
their full independent checks. On `reverb_designer.dsp`, route setup fell from
2.76 to 0.11 seconds and total vector-request time from 71.0 to 67.8 seconds.
The remaining residual classes are foreign `count`, prefix/waveform execution,
effectful FIR/IIR/table nodes, and routed-definition/transport consumption.

The next Phase 4 slice lowers Faust's canonical block-size foreign variable,
initialized `prefix` reads/writes, and direct cycling waveform literals. The
`count` case is restricted to the compute function argument and all other
foreign variables remain fail-closed. Prefix reads use the checked P6 state
cell while the write consumes either a same-loop definition or the exact value
from an accepted route. Direct waveforms emit immutable static tables, and the
final module checker requires exact coverage of those declarations before the
module is certified.

The complete corpus now has 27 certified DSPs, 64 `PureLowering` fallbacks, one
soundfile `UiProgram` fallback, and the independent SIGGEN error. The four new
paths are `bs`, `precision`, `prefix`, and `waveform1`; `precision` also became
eligible because its residual direct waveform is now supported. All four pass
the 48-case scalar/vector/native-C++ matrix formed by scalar `-ss 0..3` and
vector `-lv 0/1 x -ss 0..3`, over 60,000 samples. The full sweep showed no
compile-time regression: each new path completed in at most 0.06 seconds and
`reverb_designer` remained about 66.8 seconds, versus 67.8 seconds in the
preceding slice. Remaining table cases are effectful `rdtable`/`wrtable`
programs and stay closed until their ordered state semantics are certified.

The next Phase 4 slice adds checked read-only generated tables. The lowerer
interprets finite waveform, literal, and pure computed generators at compile
time, emits an immutable typed table, and accepts `rdtable` only when the table
is owned by that immutable declaration. Exact scalar table bounds are
preserved. Mutable `wrtable` and unknown foreign-function effects remain
explicit fail-closed boundaries rather than being admitted to a pure region.
The same slice fixes generic routed-definition and CSE identity handling; no
DSP name or corpus path participates in lowering decisions.

The complete corpus now has 44 certified DSPs, 48 fallbacks, and the existing
independent SIGGEN error. The 17 newly certified files are `BPF`, `math_simp`,
`midi_tester`, `osc`, `phasor`, `pow`, `priority`, `reverb_tester`, `select2`,
`spat`, `switcher`, `table`, `vumeter`, `waveform2`, `waveform3`, `waveform4`,
and `waveform6`. The remaining lowering boundary is specific: 13 files expose
missing routed consumer definitions, two require ordered mutable tables, and
one uses an unknown-purity foreign function. Event certification rejects 31
larger temporal programs, including 27 through a safe pre-lowering event-count
bound; the soundfile UI policy remains unchanged.

Native differential testing exposed a generic indirect recursive dependency
that the earlier fused-group producer did not close: a delayed read could pass
through an intermediate loop before reaching its recursive writer. Fused
groups now include every loop on every same-sample data path from a delayed
read to its writer, list every internal transport, and execute as one scheduling
envelope with all external incoming and outgoing dependencies. The independent
checker reconstructs data-only path closure and rejects an omitted member.
The event certificate separately models sample-major fused execution and
managed delay, recursion, prefix, and waveform transitions. Synthetic
three-loop fixtures, mutation tests, and a corpus-independent compiler fixture
cover this rule across both loop variants and all four scheduling strategies.

Compile-cost checks prevented the enlarged temporal paths from constructing an
unbounded event graph merely to reject it. A conservative lower bound computed
from roots, effects, transports, state actions, and barriers now rejects before
event materialization; `spectral_level` fell from about 29.0 to 20.7 seconds.
On `reverb_designer`, the scalar pipeline remains the dominant cost at about
61.8 seconds. A vector request takes about 68.3 seconds: the new fused-group
production and checking account for about 0.3 seconds, while approximately
5.8 seconds belongs to the pre-existing complete vector plan. Fused scheduling
dependencies are now precomputed once per epoch instead of being reconstructed
for every node. The historical source of the scalar cost remains assigned to
Phase 6.

The final Phase 4 slice corrected symbolic-recursion carrier ownership and
state-mediated zero-delay reads. `SYMREC` and `SYMREF` tuples are now explicit
checked decoration and vector-plan facts: they carry structure and aggregate
analysis facts, but no standalone runtime FIR value. Their selected scalar
bodies remain the executable units. The independent plan and route checkers
reject forged structural flags, non-tuple structural records, and missing
definitions for every non-structural signal. Variable-delay carriers whose
certified interval can include zero use checked delay state rather than an
invented value transport; any cross-loop case must be enclosed by one fused
serial group or fails closed with a specific plan diagnostic.

This moved the corpus from 44 to 49 certified DSPs. The five additions are
`capture.dsp`, `echo.dsp`, `karplus.dsp`, `quadecho.dsp`, and
`stereoecho.dsp`. The remaining waterfall is 26 bounded-event fallbacks, 13
unfused immediate-delay plan fallbacks, three intentional pure-lowering
fallbacks (two mutable tables and one unknown-purity foreign function), one
soundfile UI fallback, and one independent SIGGEN error. `smoothdelay.dsp`,
which previously failed routing, now reaches the independent event bound and
fails specifically at 4,967 events versus the current 4,096 limit.

All five newly certified files passed 60 native comparisons against the C++
60,000-frame oracle: scalar `-ss 0..3` and vector `-lv 0/1 x -ss 0..3`.
Profiling `reverb_designer` again found a stable 60--61 second scalar frontend,
about 5.83 seconds in the complete vector plan, and a 67.0 second timed vector
request. One 77.5 second wall-time sample was not reproduced and no vector
stage increased relative to the Phase 4c profile.

Phase 5 was completed on 2026-07-16. The corpus tool now records its precision
and emits portable paths plus a stable fallback reason code. A checked-in
coverage baseline stores the full float/double, `-lv 0/1`, and `-ss 0..3`
matrix: every one of its 16 modes has 49 certified files, 43 scalar fallbacks,
and the independent SIGGEN error. The per-mode residual reasons are 26 bounded
events, 13 unfused immediate-delay plans, three intentional pure-lowering
boundaries, and one soundfile UI boundary.

`vector-coverage-check` validates exact corpus/matrix completeness, each
per-mode count and fallback reason, and the generated universal certified list.
It then recompiles all 784 claimed mode/DSP pairs and requires `Certified`,
`CertifiedVector`, no fallback detail, and the independently visible
`vindex`/`vcount` chunk driver. Ubuntu CI installs Faust's standard libraries
and runs that retention check. `vec-bench` now consumes only the 49-file
universal intersection, so vector speedup aggregates cannot silently include
scalar fallback output.

The interpreter gate executes four representative certified DSPs covering
delayed recursion, structural recursion, `prefix` state, and UI lifecycle.
Across both loop variants and all four strategies, all 32 checked vector traces
match between `opt_level=0` and maximum bytecode optimization. The Phase 5
matrix itself was measured in full rather than inferred: no precision, loop,
or scheduling combination changed the 49/43/1 distribution.

## 2. Measured Baseline

The baseline was measured with the persistent corpus tool introduced by commit
`373a1afb`:

```sh
cargo run -p compiler --example count_vector_corpus
```

The tool compiles each DSP with:

```text
--double -vec -lv 0 -ss 0
```

and counts a DSP as vectorized only when the compiler reports
`VectorPipelineStatus::Certified`. This is intentionally stricter than merely
checking that `-vec` appeared on the command line.

| Outcome | DSP count | Corpus share |
|---|---:|---:|
| Certified | 3 | 3.2% |
| `UiProgram` fallback | 67 | 72.0% |
| `VectorPlan` fallback | 15 | 16.1% |
| `StatePlan` fallback | 4 | 4.3% |
| `PureLowering` fallback | 3 | 3.2% |
| SIGGEN compilation error | 1 | 1.1% |
| Total | 93 | 100% |

The certified files are:

- `logical.dsp`;
- `noiseabs.dsp`;
- `parseint.dsp`.

The independent SIGGEN failure is `subcontainer1.dsp`; it uses foreign
functions, constants, or variables that are outside this vector-coverage plan.

The current status is a first-failure waterfall. In particular, the 67 UI
programs do not reach vector planning, state planning, or pure lowering.
Supporting UI will therefore reveal additional downstream failures; it cannot
be assumed to increase the certified count by exactly 67.

## 3. Historical Analysis

### 3.1 Transitional vector implementation

Commits `07223abb` through `a9150057` introduced the initial vector-mode
plumbing and implementation:

- `ComputeMode::Vector`, `-vs`, and `-lv`;
- deterministic `LoopGraph` construction;
- loop-separation criteria;
- chunk drivers;
- signal-level loop assignment;
- cross-loop chunk buffers;
- recursive serial-core and pure-tail splitting;
- C++-compatible `-lv 0` and `-lv 1` drivers.

Commit `eb6b6044` made the recursive FIR split more conservative. Unsupported
statement shapes or unsafe dependencies remained in one fused sample loop
instead of being split. This path produced useful vector-shaped code, but it
did not carry the complete independent evidence later required by the checked
P4/P5/P6 pipeline.

### 3.2 Introduction of checked, fail-closed stages

The checked pipeline was then built incrementally:

| Commit | Relevant restriction or capability |
|---|---|
| `1ada5b26` | Production `VectorPlan`; cross-loop table carriers are rejected. |
| `2816bfee` | Verified FIR routing. |
| `4672b28f` | Pure vector lowering; unrecognized residual signal forms are rejected. |
| `6a992c9e` | Bounded fission-safety checker. |
| `58b6d330` | Verified delay/recursion state transitions; unmodelled resources are rejected. |
| `f8e9a27b` | Checked clock-domain and AD policy. |
| `d3af76e1` | Verified vector FIR region assembly. |
| `390f27d9` | Checked final-module assembly activated for the supported subset. |
| `8f701724` | Checked state and clock lowering activated. |
| `517e140c` | Clock-local state and bounded variable delays added. |

Commit `390f27d9` also introduced the unconditional UI guard at the beginning
of checked final-module construction:

```rust
if !ui_has_no_controls {
    return Err(VectorModuleFailure::new(
        VectorFallbackReason::UiProgram,
        "the checked vector module does not yet assemble grouped UI state",
    ));
}
```

At that point this did not yet force scalar output. A rejected program was
still passed to the transitional module builder with the originally requested
`ComputeMode::Vector`. `VectorPipelineStatus::Fallback` indicated that the new
producer/checker chain had not certified the result, but the fallback builder
could still emit vector-shaped code.

### 3.3 The coverage-changing commit: `b5a0a8b3`

Commit `b5a0a8b3` fixed a runtime regression exposed by:

```faust
ba = library("basics.lib");
process = ba.pulse_countup_loop(4, 1) + 0.001;
```

Under `-vec -lv 1 -ss 3`, a delayed recursive carrier could cross vector-loop
boundaries. The consumer precomputed delayed reads for the whole chunk before
the recurrence loop wrote the next state. The generated code could therefore
read stale or uninitialized short-delay storage and emit `-inf`.

The commit made two important changes:

1. it rejected unchecked cross-loop transports derived from delayed recursive
   carriers;
2. it changed every checked-pipeline fallback to use `ComputeMode::Scalar`:

```rust
let fallback_compute_mode = if vector_fallback.is_some() {
    ComputeMode::Scalar
} else {
    options.compute_mode
};
```

The first change is specific to the discovered temporal-ordering defect. The
second change is global, and is the immediate reason that all 89 current
fallbacks produce scalar modules.

This commit should not be reverted wholesale. Before it, broad apparent
vector-mode coverage included at least one known semantically unsafe shape.

### 3.4 Partial recovery in `621a82d5`

Commit `621a82d5` added a checked `FusedSerialGroup` for the direct top-rate
pattern:

```text
delayed recursive read -> recurrence writer -> safe downstream tail
```

It certifies copy-in, per-sample read/compute/write order, copy-out, scalar
internal transport, and loop membership. This restores vector mode for the
covered direct pattern without weakening the fallback. Longer chains,
multiple carriers, and clock-crossing variants remain rejected.

Commit `b5944700` subsequently fixed exponential recursive signal analysis.
That change improves compile-time behavior but does not materially broaden the
accepted vector subset.

## 4. Current Root Causes

### 4.1 UI program assembly: 67 first-stage fallbacks

The checked vector module currently assembles empty UI lifecycle bodies and
therefore rejects every non-empty `UiProgram` before analysis reaches the
other vector stages.

The scalar/transitional module already has working support for:

- DSP struct zones;
- default values and UI reset;
- metadata;
- grouped `buildUserInterface` construction;
- button and slider loads;
- bargraph stores;
- soundfile-related lifecycle state.

The missing work is checked integration, not a new Faust UI model. The vector
module must reuse or extract the existing lowering rather than duplicate a
second UI implementation.

Removing the guard without assembling this state would generate an invalid
lifecycle and incorrect runtime control behavior.

### 4.2 Vector planning: 15 fallbacks

The affected files are:

- `bs.dsp`;
- `comb_delay1.dsp`;
- `comb_delay2.dsp`;
- `math.dsp`;
- `phasor.dsp`;
- `pow.dsp`;
- `prefix.dsp`;
- `priority.dsp`;
- `table1.dsp`;
- `table2.dsp`;
- `tf_exp.dsp`;
- `waveform2.dsp`;
- `waveform3.dsp`;
- `waveform4.dsp`;
- `waveform6.dsp`.

Known plan-level restrictions include:

- table values cannot be transported between loops;
- delayed recursive carriers crossing loops must be covered by a certified
  fused serial group;
- some shared or effect-constrained graphs cannot be placed in the current
  single-epoch layout.

The public status currently retains only `VectorFallbackReason::VectorPlan`.
It discards whether a file failed because of `TableTransport`, a recursive
delay guard, missing placement, or plan verification. Exact classification is
required before changing the planner.

### 4.3 State planning: 4 fallbacks

The affected files are:

- `constant.dsp`;
- `precision.dsp`;
- `select2.dsp`;
- `waveform1.dsp`.

The state checker accepts only resources represented by the vector state plan
or the external checked clock plan. It rejects other effect resources with
`UnsupportedStateResource`, and also checks loop ownership, recursion identity,
clock ownership, and delay geometry.

The category alone is insufficient to claim that all four files fail for the
same resource. The detailed `VectorStateError` must be preserved and measured
before implementing a new transition model.

### 4.4 Pure lowering: 3 fallbacks

The affected files are:

- `math_simp.dsp`;
- `par_fir_32.dsp`;
- `priority1.dsp`.

The pure lowerer already handles common arithmetic, casts, selections, delays,
recursion, projections, and math calls. Remaining failures can arise from an
unsupported signal node, an effectful signal in a pure region, a type mismatch,
missing routed evidence, or invalid control dependence. As with plan and state
failures, the retained status is not detailed enough to choose a correct fix.

### 4.5 Test observability gap

Earlier differential matrices primarily proved output parity. They did not
systematically require `VectorPipelineStatus::Certified` or inspect the final
FIR shape. After `b5a0a8b3`, a correct scalar fallback naturally passes a
scalar-versus-`-vec` output comparison.

Correctness parity and vector-retention coverage are separate gates. Both are
required.

## 5. Correction Plan

### Phase 0 - Preserve complete fallback diagnostics

Goal: make every corpus rejection actionable before broadening semantics.

Actions:

1. Extend the observable pipeline result so a fallback retains both the stable
   reason and `VectorModuleFailure.detail`.
2. Keep stable reason codes for snapshots and CI, while exposing structured
   stage-specific errors where practical.
3. Extend `count_vector_corpus` with:
   - detailed error text;
   - counts by concrete error variant;
   - optional per-stage waterfall analysis;
   - machine-readable JSON or TSV output for CI comparison.
4. Record separately whether the emitted module is certified vector, legacy
   vector, or scalar fallback. Do not infer this solely from the requested CLI
   options.

Acceptance criteria:

- every one of the 89 fallbacks has a concrete diagnostic;
- the totals remain reproducible and sum to the corpus size;
- diagnostic additions do not alter generated FIR or runtime output.

### Phase 1 - Integrate UI into the checked vector module

Goal: remove the unconditional `UiProgram` rejection for ordinary controls and
bargraphs while preserving the Faust lifecycle contract.

Actions:

1. Extract reusable UI/lifecycle construction from
   `signal_fir/module/ui_lowering.rs` instead of creating a divergent vector
   implementation.
2. Add UI zone declarations to the DSP struct.
3. Populate `instanceResetUserInterface` with the accepted defaults.
4. Emit grouped `buildUserInterface` operations and metadata in canonical
   order.
5. Lower control reads at the correct control-rate point before dependent
   vector loops.
6. Place bargraph writes after their producing computation while preserving
   effect order.
7. Add a producer/checker artifact that proves exact zone coverage, unique
   ownership, lifecycle placement, and UI read/write ordering.
8. Treat soundfiles as a separate subphase if their lifecycle cannot be
   certified with ordinary controls.

Acceptance criteria:

- representative button, checkbox, slider, numerical-entry, bargraph, nested
  group, metadata, and multi-control DSPs report `Certified`;
- scalar and vector interpreter outputs are bit-exact where applicable;
- C and C++ generated modules pass lifecycle and runtime differential tests;
- unsupported soundfile forms retain a specific, observable fallback;
- removing UI fallback does not weaken state, clock, or effect certificates.

### Phase 2 - Recover vector-plan coverage

Goal: address the 15 current plan failures using exact diagnostics.

Actions:

1. Partition the files into table transport, recursive-delay crossing,
   placement, and verification failures.
2. For table carriers, choose and certify one of these representations:
   - force producer and consumers into a common owner loop;
   - pass a stable table handle when only identity crosses the boundary;
   - introduce an explicit table transport record with lifetime and mutation
     rules.
3. Generalize `FusedSerialGroup` beyond the direct slice to supported longer
   chains and multiple internal transports.
4. Add multi-carrier or clock-crossing fusion only after defining exact
   temporal and ownership invariants; otherwise retain scalar fallback.
5. Improve placement only when the independent checker can reconstruct and
   validate the chosen ownership from decorations.

Acceptance criteria:

- each newly supported class has producer, independent checker, mutation, FIR
  structure, and scalar/vector differential tests;
- no delayed recursive read is precomputed before its same-chunk state writes;
- table lifetime and mutation semantics match the C++ reference;
- `-ss 0..3` changes only legal scheduling choices, not semantics.

### Phase 3 - Extend checked state transitions

Goal: model the concrete missing state resources found in the four current
state-plan failures.

Actions:

1. Classify each failure using Phase 0 diagnostics.
2. Add one resource class at a time to `VectorStatePlan`.
3. Define its `pre`, per-sample `exec`, and `post` transitions and lifecycle
   ownership.
4. Extend the event-order certificate and assembly checker for the resource.
5. Test both short blocks and chunk tails, including `count < vec_size`.

Acceptance criteria:

- every accepted resource has exact transition and lifecycle evidence;
- unmodelled resources continue to fail closed;
- optimized and unoptimized interpreter executions agree;
- scalar/vector/C++ differential tests cover state across chunk boundaries.

### Phase 4 - Complete the residual pure lowerer

Goal: support the exact residual forms blocking the three current pure-lowering
files.

Actions:

1. Use detailed diagnostics and minimized DSP fixtures to identify each form.
2. Add canonical lowering and type checks for supported pure forms.
3. Keep effectful or control-dependent forms out of pure regions unless routing
   evidence is extended first.
4. Add structural tests for CSE root coverage and region ownership.

Acceptance criteria:

- the three existing files either certify or report a more specific intentional
  unsupported category;
- no new lowering path bypasses effect, type, or route verification;
- generated results match scalar and C++ references.

### Phase 5 - Add vector-retention quality gates

Goal: prevent correct-but-scalar fallback from being mistaken for vector-mode
coverage.

Actions:

1. Run the corpus counter across:
   - `-lv 0` and `-lv 1`;
   - `-ss 0..3`;
   - float and double precision where practical.
2. Store a repository-relative coverage summary with certified, fallback, and
   error counts.
3. Fail CI when:
   - a previously certified DSP becomes scalar fallback without an approved
     baseline update;
   - the report is incomplete;
   - a claimed certified module lacks the expected chunk-driver/vector-region
     structure.
4. Keep differential correctness gates for all supported backends.
5. Compare interpreter `opt_level=0` and maximum optimization on a
   representative vector subset.
6. Benchmark certified modules separately from scalar fallbacks.

Acceptance criteria:

- CI reports effective vector retention independently from numerical parity;
- coverage changes are reviewable per DSP and per fallback reason;
- benchmark summaries never include scalar fallback under vector speedup
  aggregates.

### Phase 6 - Audit scalar compilation history and cost

Goal: determine whether the expensive scalar corpus cases predate scheduling
strategies and checked vectorization, and remove any demonstrated regression.

Actions:

1. Identify the commits immediately before `-ss` activation and vector-mode
   activation, then build comparable release binaries from isolated worktrees.
2. Measure a fixed slow-DSP subset and the corpus aggregate with identical
   frontend, precision, include paths, FIR lane, and output mode.
3. Add stage timing around parse/import, evaluation, propagation,
   normalization, preparation, scheduling, FIR lowering, and backend emission
   so absolute scalar cost is attributable rather than inferred from total
   wall time.
4. Classify each cost as historical, scheduling-related, vector-analysis
   leakage into scalar mode, or another later regression.
5. Fix every reproducible regression, add a structural or growth test for its
   cause, and retain a benchmark baseline that can detect recurrence.

Acceptance criteria:

- the comparison names exact commits, commands, machine context, and repeated
  measurements;
- scalar mode does not execute vector-only certification work;
- demonstrated post-baseline regressions are corrected or retained only with
  an explicit measured justification;
- the final report separates historical frontend cost from `-ss` and vector
  mode overhead.

Completed on 2026-07-16. The release-history audit is recorded in
[`scalar-compilation-history-and-cost-audit-2026-07-16-en.md`](scalar-compilation-history-and-cost-audit-2026-07-16-en.md).
It confirms that the slow scalar cases were evaluation-bound before both
feature activations, removes scalar construction of vector occurrence/condition
facts, and reduces effect-conflict orientation from a quadratic all-pairs scan
to resource-grouped constraints. Scalar mode is structurally tested not to run
vector certification; the remaining measured FIR delta is explicit verified
preparation/normalization work.

## 6. Recommended Execution Order

The recommended order is:

```text
detailed diagnostics
    -> ordinary UI controls and bargraphs
    -> remeasure the full waterfall
    -> table and recursive transport classes
    -> missing state resources
    -> residual pure lowering
    -> permanent retention and performance gates
    -> scalar compilation history and performance audit
```

UI is the highest-leverage implementation area because it currently blocks
72% of the corpus at the first checked stage. Diagnostics come first because
the downstream distribution will change once UI programs can proceed further.

The first coverage milestone should not promise a fixed certified count. It
should require that UI-bearing, otherwise stateless and pointwise DSPs certify,
then use the new waterfall to set the next evidence-based target.

## 7. Safety and Compatibility Rules

- Do not remove a fallback merely to increase the certified count.
- Do not restore the pre-`b5a0a8b3` transitional vector fallback globally.
- Keep the delayed-recursive transport guard authoritative unless an accepted
  fused-group certificate covers the exact dependency.
- Preserve the Faust lifecycle contract for UI and persistent state.
- Preserve the external CLI behavior of `-vec`, `-lv`, `-vs`, and `-ss`.
- Keep public API changes additive or adapted with documented compatibility
  impact.
- Use repository-relative paths in stored reports and fixtures.
- Treat corpus coverage as evidence of implementation breadth, not as a proof
  of semantic correctness; producer/checker and differential gates remain
  authoritative.

## 8. Completion Criteria

This recovery plan is complete when:

1. every requested vector compilation reports both its effective mode and a
   detailed certified/fallback reason;
2. ordinary UI programs use the checked vector pipeline;
3. the current plan, state, and lowering failures have been classified and
   either implemented with evidence or retained as explicit supported-policy
   fallbacks;
4. scalar fallback cannot silently pass as vector retention in CI or
   benchmarks;
5. the full `-lv 0/1 x -ss 0..3` matrix preserves scalar and C++ parity;
6. no known unsafe recursive-delay transport is accepted without a verified
   fused serial group.
