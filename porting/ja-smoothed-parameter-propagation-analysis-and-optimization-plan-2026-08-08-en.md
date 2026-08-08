# Jiles-Atherton Smoothed-Parameter Propagation Cost — Analysis and Optimization Plan

**Date**: 2026-08-08
**Status**: revised after direct C++ propagation profiling; nothing implemented
**Primary cases**:

- `hysteresis_tests.dsp::ja_processor_stereo_ui_test`
- `demos_tests.dsp::ja_transformer_demo_test`

**Related**:

- `porting/propagation-cost-analysis-2026-08-06-en.md`
- `porting/eval-box-simplification-memoization-analysis-2026-08-06-en.md`
- `porting/compile-time-provenance-regression-analysis-and-plan-2026-07-30-en.md`
- `porting/cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md`

---

## 1. Executive summary

The two slow cases are not independent. They expose the same propagation shape:

1. a UI parameter is turned into a recursive signal by `si.smoo`;
2. that signal is passed as `Ms` to the recursive Jiles-Atherton model;
3. `Ms` contributes to expressions reused throughout four cascaded substeps;
4. the complete model is duplicated by `par(i, 2, ...)`.

On the current release build, both cases spend about **0.95 s in faust-rs
propagation**. C++ Faust reports about **0.6 ms** in its propagation stage for
the same inputs. Stage boundaries differ between the implementations, so that
ratio is not itself an optimization target, but the faust-rs attribution is
unambiguous: parser, evaluation, signal-to-FIR, and code generation are not the
source of the one-second cost.

A differential probe isolates the trigger. In the stereo processor, changing
only `Ms` from a raw slider signal to the same slider followed by `si.smoo`
raises propagation from about **0.49 s to 0.90 s**. Enabling every other UI
parameter adds almost nothing beyond the smoothed `Ms`. The transformer shows
the same transition, from about **0.51 s to 0.90 s**.

This is a compiler optimization problem. The DSP source is valid, the smoothing
has runtime semantics, and the plan below requires byte-identical generated
output and unchanged diagnostics. Source-level removal or relocation of the
smoothing is explicitly outside the scope of this document.

Inspection and profiling of C++ Faust changes the leading implementation
hypothesis. C++ does use an exact whole-propagation result memo. It can afford
that memo because the Box, slot environment, UI path, and input signals are all
hash-consed `Tree*` values: an entire context is represented and compared by a
few canonical pointers. The previously rejected Rust memo used a materially
more expensive key built around a mutable `AHashMap` slot environment and owned
signal buses. That experiment refutes that representation, not result
memoization itself. The revised plan therefore makes compact canonical context
identities a prerequisite to retesting a C++-style propagation cache.

## 2. Measurement method

The measurements use the current release CLI, the local faustlibraries checkout,
and the same per-symbol selection used by `xtask examples-compare`:

```sh
faust-rs -time \
  -I /Users/letz/Developpements/faustlibraries \
  -pn ja_processor_stereo_ui_test \
  -o /tmp/ja_processor_stereo_ui.cpp \
  /Users/letz/Developpements/faustlibraries/tests/hysteresis_tests.dsp

faust-rs -time \
  -I /Users/letz/Developpements/faustlibraries \
  -pn ja_transformer_demo_test \
  -o /tmp/ja_transformer_demo.cpp \
  /Users/letz/Developpements/faustlibraries/tests/demos_tests.dsp
```

`examples-compare` launches each compiler as a fresh process, repeats each case,
and retains the minimum. Process startup and repeated library loading contribute
to the corpus total, but they do not explain these cases: the extra time is
inside the measured propagation phase.

The isolation probes keep the model topology and UI expressions unchanged while
varying only the expression supplied as `Ms`:

- constant `380`;
- raw slider;
- the same slider followed by `si.smoo`.

Times below are representative release runs. Before setting or updating a
performance budget, the retained fixtures must be measured with warm-up and at
least five timed runs, keeping the minimum to reject scheduler interference.

## 3. Primary stage attribution

| DSP | evaluation | arity | propagation | signal/FIR | total |
|---|---:|---:|---:|---:|---:|
| `ja_processor_stereo_ui_test` | 67 ms | 155 ms | **958 ms** | 11 ms | 1.20 s |
| `ja_transformer_demo_test` | 43 ms | 82 ms | **947 ms** | 11 ms | 1.09 s |

C++ Faust reference measurements:

| DSP | evaluation | propagation | code generation |
|---|---:|---:|---:|
| `ja_processor_stereo_ui_test` | 105 ms | **0.52 ms** | 10 ms |
| `ja_transformer_demo_test` | 65 ms | **0.63 ms** | 12 ms |

The C++ stage names do not define an architectural equivalence with the Rust
pipeline. They do establish that the source shape does not inherently require a
one-second signal propagation pass.

## 4. Structural scaling of the stereo UI processor

The existing faustlibraries test symbols provide a natural scaling sequence:

| variant | faust-rs propagation | total |
|---|---:|---:|
| `ja_hysteresis_test` | 198 ms | 260 ms |
| `ja_processor_test` | 201 ms | 285 ms |
| `ja_processor_stereo_test` | 402 ms | 528 ms |
| `ja_processor_ui_test` | 480 ms | 607 ms |
| `ja_processor_stereo_ui_test` | **957 ms** | 1.17 s |

The stereo wrapper almost exactly doubles the mono cost. The UI wrapper makes
the mono case about 2.4 times as expensive as the corresponding processor with
constant parameters. Combining both multipliers predicts the measured
0.95-second propagation cost.

This rules out a general code-generation problem and points to work repeated
along structural composition paths.

## 5. Parameter-by-parameter isolation

The stereo processor was compiled with one dynamic UI argument enabled at a
time. `base` uses constants for all processor parameters.

| dynamic argument | arity | propagation | total |
|---|---:|---:|---:|
| none (`base`) | 74 ms | 432 ms | 560 ms |
| `Ms : si.smoo` | **156 ms** | **894 ms** | **1.11 s** |
| `a` | 93 ms | 504 ms | 646 ms |
| `alpha` | 75 ms | 417 ms | 540 ms |
| `k` | 78 ms | 413 ms | 545 ms |
| `c` | 74 ms | 412 ms | 536 ms |
| `drive : db2linear : si.smoo` | 76 ms | 405 ms | 535 ms |
| `trim : db2linear` | 75 ms | 401 ms | 526 ms |
| all core UI arguments | 149 ms | 903 ms | 1.11 s |
| drive and trim UI arguments | 75 ms | 410 ms | 537 ms |
| all UI arguments | 156 ms | 911 ms | 1.12 s |

`Ms` accounts for nearly the entire UI-related increase. The result is not
caused by smoothing alone: the smoothed drive parameter is cheap in this model.
The expensive conjunction is a recursive smoothed argument used deeply and
repeatedly by another recursive graph.

A second differential replaces only the smoothed `Ms` with the same raw slider:

| `Ms` expression | arity | propagation | total |
|---|---:|---:|---:|
| raw slider | 90 ms | 491 ms | 651 ms |
| raw slider followed by `si.smoo` | **151 ms** | **899 ms** | **1.10 s** |

## 6. Transformer confirmation

The transformer includes pre-EQ, post-EQ, dry/wet mixing, gain staging, and the
same Jiles-Atherton core. Holding every other expression constant and changing
only `Ms` gives:

| `Ms` expression | arity | propagation | signal/FIR | total |
|---|---:|---:|---:|---:|
| constant | 44 ms | 472 ms | 30 ms | 628 ms |
| raw slider | 47 ms | 509 ms | 21 ms | 614 ms |
| raw slider followed by `si.smoo` | **76 ms** | **904 ms** | 22 ms | **1.04 s** |

The extra EQ and mixing graph contributes to the baseline, but it is not the
cause of the jump to one second. The same smoothed-`Ms` shape adds about 0.4 s.

## 7. Why this source shape is expensive

### 7.1 `si.smoo` is a recursive signal

`si.smoo` is implemented through `si.smooth`, whose selected form is:

```faust
fb(y) = (1.0 - s) * x + s * y;
```

It therefore introduces its own feedback group. A slider is a leaf-like signal;
the smoothed slider is a recurrent signal graph with state and De Bruijn
recursion structure.

### 7.2 `Ms` has high semantic fan-out inside Jiles-Atherton

The Jiles-Atherton model derives several expressions from `Ms`:

```faust
Ms_safe = max(Ms, 1e-6);
a_norm = a / Ms_safe;
k_norm = k / Ms_safe;
inv_a_norm = 1.0 / max(a_norm, 1e-9);
```

The derived values feed the nonlinear `substep`, and the model invokes four
cascaded substeps inside its feedback loop. The complete processor is then used
twice by the stereo `par` expression.

The final generated C++ barely changes in the differential probes:

| probe | raw `Ms` | smoothed `Ms` | increase |
|---|---:|---:|---:|
| stereo processor | 18,569 bytes | 19,022 bytes | 453 bytes |
| transformer isolation | 44,096 bytes | 44,367 bytes | 271 bytes |

The roughly 80–100% propagation-time increase is therefore not proportional to
the final program. It is repeated intermediate compiler work that is mostly
collapsed by the time code is emitted.

### 7.3 Relevant faust-rs behavior

`propagate_in_slot_env` currently performs, for every recursive entry:

1. a memoized arity lookup;
2. recursive lowering through `propagate_inner`;
3. output-width validation;
4. a forward-AD structural predicate on non-exact widths;
5. provenance recording through `record_derived_forest`;
6. construction and return of a fresh `Vec<SigId>` bus.

Entering a Box recursion also lifts all values in `slot_env` into the new De
Bruijn scope. A complex recurrent control signal is therefore a materially
different input from a raw widget leaf.

The earlier propagation-cost investigation measured the same broader symptom on
another UI-heavy DSP: tens of millions of `propagate_inner` calls, with 83% of
returned buses containing at most one signal. `mimalloc` reduced allocator cost
for the CLI but did not reduce the amount of work, and library/FFI users do not
inherit that allocator choice.

The current measurements do not yet prove which sub-operation dominates these
two Jiles-Atherton cases. They prove the stage, the source trigger, the structural
multipliers, and the mismatch between intermediate cost and final graph size.
Function-level attribution is the first phase of the plan below.

### 7.4 Why C++ Faust completes propagation in about one millisecond

C++ Faust's performance is not a special-case simplification of
Jiles-Atherton. It follows from a propagation representation designed around
canonical identity.

#### Maximally shared trees

Box and Signal expressions are immutable, hash-consed `CTree` objects. Pointer
identity is structural identity. A repeated `Ms : si.smoo` expression and a
repeated derived signal therefore reuse canonical pointers rather than requiring
deep equality or reconstruction.

#### Exact propagation-result memo

Every C++ `propagate()` call probes a compilation-scoped memo whose logical key
is:

```cpp
struct PropagateMemoKey {
    Tree    fSlotEnv;
    Tree    fPath;
    Tree    fBox;
    siglist fInputs;
};
```

`fSlotEnv`, `fPath`, `fBox`, and every element of `fInputs` are canonical tree
pointers. The complete environment and UI path each hash and compare as one
pointer. A hit returns an already-built `siglist` and skips the complete
sub-propagation.

With `FAUST_PROPAGATE_PROFILE=1`, the two primary cases report:

| DSP | calls | hits | hit rate | profiled propagation |
|---|---:|---:|---:|---:|
| `ja_processor_stereo_ui_test` | 2,431 | 427 | 17.6% | 0.96 ms |
| `ja_transformer_demo_test` | 2,476 | 353 | 14.3% | 1.64 ms |

The hit rate understates the saved work because one hit on a composition node
skips its entire descendant traversal. The processor has 166 `seq` hits and the
transformer 141. Profiling itself approximately doubles the sub-millisecond
unprofiled phase, but the absolute cost remains tiny.

#### O(1) arity and De Bruijn aperture

C++ `getBoxType()` attaches the computed arity directly to the canonical Box as
a property. Subsequent queries retrieve it without a separate hash lookup keyed
from outside the node.

Each `CTree` also stores its synthesized De Bruijn aperture. `liftn()` begins
with `isClosed(t)` and returns the original pointer immediately for a closed
signal. The `si.smoo` recursion is closed relative to the surrounding
Jiles-Atherton recursion, so lifting it into that scope does not require walking
its signal graph.

faust-rs memoizes `liftn` and aperture externally, but the first query still
walks the graph and populates maps. Whether that difference is material on these
two cases must be measured; it is a concrete difference from C++, not yet a
proven dominant cost.

#### Less propagation-side diagnostic bookkeeping

C++ propagation does not perform the faust-rs `record_derived_forest` operation
after every validated Box boundary. Better Rust diagnostics are a valid feature,
but their cost is additional work that the C++ timing does not contain.

### 7.5 Consequence for the optimization direction

The relevant comparison is not "C++ has a memo and Rust does not" in isolation.
It is:

```text
C++: canonical persistent context -> cheap exact key -> profitable result memo
Rust: mutable/non-canonical context -> expensive key -> unprofitable memo attempt
```

The missing prerequisite in faust-rs is a cheap semantic identity for the full
propagation context. Retesting result memoization before introducing that
identity would repeat the failed experiment. Declaring result memoization
categorically unsuitable would ignore the measured C++ implementation.

## 8. Constraints learned from refuted Rust implementations

The following experiments constrain the revised design. They do not overturn
the C++ evidence; they identify representations that must not be repeated.

### 8.1 Whole-call memo with a non-canonical Rust context key

The attempted memo increased propagation from 10.6 s to 13.7–13.9 s on the
reference case. With a complete environment-sensitive key it achieved only a
12% hit rate and retained about 748,000 `Vec<SigId>` entries. It added hashing,
memory, and ownership cost with the wrong representation.

This does **not** refute C++-style whole-call memoization after context
canonicalization. In C++, the equivalent slot environment and path are already
single canonical pointers, while the Rust experiment had to represent or hash
the contents of mutable structures. A new memo experiment is justified only
after its context key has O(1) identity and its result storage does not retain an
owned `Vec` per entry.

### 8.2 `SmallVec<[SigId; 2]>` as a mechanical replacement

Although 98% of measured buses had length at most two, `SmallVec` was slower.
Propagation results are moved and dropped so frequently that the inline/spilled
branch cost outweighed the saved tiny allocations.

### 8.3 Standalone closed-subterm `liftn` fast path

The proposed fast path was also measured and rejected. Existing `liftn`
memoization handles almost all calls in the measured corpus before the new guard
would help. C++ nevertheless has an O(1) closed-tree test because aperture is a
synthesized node field. The rejected experiment says not to add another hashmap
probe or graph computation before Rust `liftn`; it does not rule out making
aperture cheaply available as part of a broader dense/canonical node plan.

### 8.4 Allocator-only treatment

`mimalloc` is a valid CLI-level mitigation already in place. It cannot be the
structural fix: it leaves the repeated result construction intact and does not
help consumers that embed the compiler library with another allocator.

## 9. Optimization objective and constraints

The objective is to make propagation cost follow the size of the distinct
semantic graph rather than the number of composition paths that revisit it.

An acceptable optimization must:

- preserve signal, FIR, and generated C++ output;
- preserve UI paths, control identities, clock domains, and diagnostics;
- preserve forward-AD and recursive-signal semantics;
- work for the compiler library as well as the mimalloc-backed CLI;
- improve both primary Jiles-Atherton cases;
- avoid a material regression on the full compile corpus;
- bound additional peak memory.

Initial success target:

- reduce each primary case's propagation time by at least **2×**;
- make the smoothed- versus raw-`Ms` propagation delta no greater than 25% of
  the raw case;
- keep aggregate `examples-compare` faust-rs time within 2% of baseline or
  improve it;
- keep peak memory within 10% of baseline.

The C++ propagation time is useful comparative evidence but is not the first
milestone because the pipelines assign work to phases differently.

## 10. Phased optimization plan

### P0 — Retain representative benchmarks before changing propagation

**Implementation status (2026-08-08): complete.** The self-contained
`tests/compile-budget/dsp/ja_smoothed_parameter.dsp` fixture retains the four
substeps and stereo shape without importing faustlibraries. Three reduced cores
in series reproduce the original absolute differential while keeping the source
portable: release propagation measured 0.873 s for raw stereo and 1.284 s for
smoothed stereo, a 0.410 s delta. The smoothed stereo `process` is a required
normalized front-end budget case; the raw/smooth and mono/stereo definitions
remain selectable with `-pn` for differential profiling.

Add two kinds of performance input:

1. optional full-library cases for the two named faustlibraries symbols;
2. a self-contained reduced DSP in the repository that preserves all three
   relevant axes: smoothed recursive argument, fourfold use inside recursion,
   and stereo duplication.

The reduced fixture should expose at least these symbols:

- constant parameter, mono;
- raw dynamic parameter, mono;
- smoothed dynamic parameter, mono;
- raw dynamic parameter, stereo;
- smoothed dynamic parameter, stereo.

Add the smoothed stereo case to the normalized front-end basket in
`compile-budget-check`. Keep the whole scaling matrix in a dedicated benchmark
or profiler harness so a change that helps only one endpoint cannot hide a
regression in another.

Record baseline wall time, stage share, peak RSS, and deterministic operation
counts. Wall time alone is too noisy for diagnosis.

**P0 exit criterion**: the reduced fixture reproduces at least 75% of the
smoothed/raw propagation delta and its stage is at least 70% propagation.

### P1 — Add temporary propagation attribution counters

**Implementation status (2026-08-08): complete.** The propagate crate now
implements opt-in `FAUST_PROPAGATE_PROFILE` attribution with C++-comparable Box
families plus Rust-specific bus, slot, lifting, and origin counters. On the
retained sentinel, smoothed stereo executes 10,836,611 Rust propagation calls
against 3,803 in C++; raw stereo executes 8,009,171 against 2,939. Profiled
origin attribution costs 0.538 of 1.980 seconds (27%) for smoothed stereo and
0.412 of 1.424 seconds (29%) for raw stereo. Smoothed `liftn` performs 473,425
calls with a 99.8% memo hit rate. The leading difference is therefore millions
of avoidable propagation entries; provenance is a material residue, while a
standalone `liftn` algorithm change cannot remove the dominant factor.

Instrument one compilation with counters scoped to `PropagateContext`:

- `propagate_inner` calls by `FlatNodeKind`;
- visits per `FlatBoxId` and the distribution of repeat counts;
- result-bus length histogram and total capacity allocated;
- `box_arity_typed` queries and cache hits;
- `match_box` and `flat_node_kind` calls;
- `slot_env` size distribution;
- `liftn` calls, memo hits, and nodes cloned;
- recurrence entries and number/complexity of lifted slot values;
- `record_derived_forest` calls, nodes visited, and already-attributed prunes;
- time or operation counts attributable to the smoothed parameter's reachable
  Box and Signal subgraphs;
- cardinality and construction cost of the exact semantic context that a result
  memo would require: slot bindings, UI path, clock state, AD mode, and inputs;
- projected cache hits under canonical identity, recorded without storing
  results, so key usefulness is separated from result-retention cost.

Run the same counters on constant, raw, and smoothed variants. The useful number
is the differential, not the absolute total.

Perform two diagnostic-only experiments, both reverted after measurement:

1. propagation with origins disabled, to bound provenance cost on these exact
   cases;
2. propagation with redundant contract checks counted but bypassed only in an
   internal trusted path, to bound validation/hash cost. This experiment must
   never become the public path without a proof that validation remains at the
   boundary.

Run C++ Faust with `FAUST_PROPAGATE_PROFILE=1` on the same reduced variants and
report the same per-kind calls/hits/misses table. The Rust instrumentation should
make the comparison meaningful enough to answer whether Rust performs more
semantic propagation calls, pays more per call, or both.

**P1 exit criterion**: at least 80% of the smoothed/raw operation-count delta is
assigned to named operations or repeat-visit classes, and the projected exact
memo hit rate is known independently of key construction and result storage. Do
not select a structural fix from wall-clock samples alone.

### P2 — Model canonical propagation identity and locate repeated work

The failed Rust whole-call memo proves that its concrete key and retained result
representation were unprofitable. It does not prove that the semantic key
`(box, inputs, complete context)` has insufficient reuse when each component has
cheap canonical identity. C++ uses that exact strategy successfully.

First specify the Rust semantic key before implementing it. At minimum it must
distinguish:

- `FlatBoxId`;
- input signal bus;
- slot environment;
- normalized UI group path/control identity context;
- clock environment and clock domain;
- `suppress_fad` and any other propagation mode that changes outputs;
- recursion/De Bruijn scope where it is not already encoded in canonical input
  signals.

Pending FAD side effects and any mutable diagnostic side channel must either be
represented in the key/result protocol or make the call ineligible for caching.
The cache must never silently replay a signal result while omitting a required
side effect.

Then use P1 data to test these hypotheses:

1. **Canonical-context reuse**: estimate hits for an exact semantic key while
   representing each context component by an interned ID. Compare this directly
   with the C++ per-kind profile.
2. **Repeated flat decoding and arity work**: determine whether the same
   `FlatBoxId` is repeatedly decoded and hash-looked-up despite immutable kind
   and arity.
3. **Repeated lowering of shared pure fragments**: identify nodes with high
   fan-out whose result is context-independent for the current recursion,
   clock, UI path, and AD state.
4. **Recurrent slot lifting**: measure whether the smoothed signal's De Bruijn
   graph is cloned or walked repeatedly when entering the Jiles-Atherton `Rec`.
5. **Bus materialization**: distinguish time spent creating propagation results
   from time spent building distinct signal nodes.
6. **Provenance closure**: determine whether hash-consed outputs are cheap to
   build but expensive to attribute repeatedly.

For each high-fan-out node, record both visit count and number of distinct output
signal IDs. A node visited many times but producing the same IDs indicates
avoidable traversal. A node producing different IDs indicates context-sensitive
work and rules out unsafe sharing.

**P2 exit criterion**: a reduced trace explains why the raw widget leaf is cheap
and its smoothed recurrent replacement adds approximately 0.4 s; the complete
semantic cache key is specified; and a counter-only canonical-key simulation
predicts enough subtree reuse to amortize lookup and storage.

### P3 — Introduce cheap canonical propagation context identities

This is the new prerequisite revealed by the C++ comparison. Implement and
benchmark the context pieces independently before adding a result memo. Each
canonicalization patch must be semantics-preserving and must be retained only if
it is neutral or independently beneficial when measured without the subsequent
cache.

#### P3-A — Persistent canonical slot environments

Replace the mutable `AHashMap<BoxId, SigId>` identity problem with a
compilation-local persistent representation, conceptually:

```text
SlotEnvId -> Empty
          | Bind(parent: SlotEnvId, slot: BoxId, signal: SigId)
```

Intern `Bind` nodes so identical environments receive the same `SlotEnvId`.
Preserve lexical shadowing exactly. Environments are measured to be small, so a
short parent-chain lookup may be cheaper than cloning or hashing a map; confirm
that on the corpus rather than assuming it.

Entering `Rec` currently rebuilds a map after lifting every value. Add an
environment-lift memo keyed by `(SlotEnvId, threshold)` or derive a new interned
environment whose unchanged closed values retain identity. Never reuse an
environment across incompatible De Bruijn scopes.

#### P3-B — Canonical UI path and mode identity

Represent the normalized UI group path as an interned persistent path:

```text
UiPathId -> Root | Segment(parent: UiPathId, normalized_segment)
```

Keep control-ID semantics and navigation normalization unchanged. Form a compact
`PropagationModeKey` for the remaining output-affecting state, including clock
environment/domain and AD suppression mode. Calls with pending side effects
that cannot be represented safely must be marked non-cacheable.

#### P3-C — Compact input and output bus identity

Do not retain one owned `Vec<SigId>` in every memo entry again. Evaluate:

- a compilation-local immutable bus arena returning `BusId`;
- interned bus slices only at candidate cache boundaries;
- a fixed-size key containing length plus a cached exact hash, with equality
  against arena storage;
- direct small-bus fields for the overwhelmingly common zero-, one-, and
  two-signal keys without changing recursive return values globally.

C++ still hashes its input vector elements, so complete bus interning is not a
required parity feature. The requirement is that a cache probe not allocate or
clone an owned bus.

#### P3-D — Dense immutable Box facts

If P1 attributes meaningful cost to flat decoding, arity, aperture, or
forward-AD containment, attach these immutable facts to a dense plan indexed by
`FlatBoxId`/`TreeId`:

- decoded `FlatNodeKind` and child IDs;
- input/output arity;
- De Bruijn aperture or closedness;
- static context-sensitivity and forward-AD flags.

This mirrors C++ node properties and synthesized aperture without adding a new
hash lookup to the hot path. It is not required for the result memo unless the
measurements show that canonical context alone leaves these lookups dominant.

**P3 exit criterion**: the exact propagation context can be represented by a
small fixed-size key containing canonical IDs; constructing/probing that key
allocates nothing; canonicalization alone does not regress the aggregate corpus
by more than 2%; and semantic tests prove that distinct UI, clock, recursion,
and AD contexts never alias.

### P4 — Retest exact propagation-result memoization with canonical keys

After P3, add a compilation-scoped memo equivalent in semantics to the C++
table:

```text
PropagateKey {
    box: FlatBoxId,
    slot_env: SlotEnvId,
    ui_path: UiPathId,
    mode: PropagationModeKey,
    inputs: BusKey or BusId,
} -> BusId
```

The cache lifetime remains one top-level propagation. No entry may survive into
another `TreeArena` or compilation.

Benchmark these activation policies separately:

1. cache every eligible call, matching C++;
2. insert only for Box nodes with structural fan-out greater than one;
3. allocate a result entry only on the second observed key visit;
4. cache composition nodes (`seq`, `par`, groups, recursion) before atoms, since
   the C++ profile shows that a composition hit skips the most work.

Start with the exact complete key, not a context-free approximation. Optimize
eligibility only after correctness and hit data are available.

#### Side effects and provenance on cache hits

`propagate_inner` is not purely a `Box -> signals` function in faust-rs. It also
updates pending FAD state, clock tables, UI/control state, and signal origins.
For each side effect, choose and test one explicit policy:

- encode the state in the key and replay a compact cached delta;
- move deterministic side-effect construction outside the memoized signal core;
- mark the call non-cacheable;
- prove that the first miss has already installed compilation-global data that
  makes a later exact-context hit observationally inert.

In particular, returning cached signals must not omit the more-specific child
origins that diagnostics expect. Since the cache is per traversal, a prior miss
may already have populated those origins, but origin ordering and multi-origin
selection must be covered by mutation tests rather than assumed.

#### Profitability gates

Record calls, hits, misses, entries, retained bus words, lookup time, and avoided
`propagate_inner` calls. Compare against C++ per-kind data. A modest hit rate can
still be profitable when hits occur high in a composition tree; raw hit rate is
not sufficient to accept or reject the cache.

**P4 exit criterion**: the canonical-key memo reduces both primary cases by at
least 2×, avoids at least 75% of the smoothed/raw delta, produces identical
signals/FIR/C++, keeps peak RSS within 10%, and does not regress aggregate
`examples-compare` time by more than 2%.

If a canonical-key memo still fails, retain the P3 representation only if it is
neutral or independently beneficial, and use the P1/P2 attribution to select a
narrower structural optimization such as caller-owned buses or fused
composition propagation. Do not fall back to a non-canonical memo key.

### P5 — Reduce provenance overhead without weakening diagnostics

Treat provenance separately from core propagation so its benefit remains
measurable. If P1 shows a material residue, compare:

1. compact parent/origin links recorded at signal construction;
2. lazy origin closure built only when a diagnostic requests it;
3. direct attribution of newly interned nodes instead of a forest walk after
   each Box propagation;
4. retention of the current bounded eight candidates only at diagnostic
   materialization time.

The optimized design must reproduce existing diagnostic snapshots, including
the selected source occurrence when a hash-consed signal has several candidate
Box origins. Disabling provenance in successful CLI compilations is not an
acceptable shortcut unless all error paths can reconstruct identical evidence.

**P5 exit criterion**: diagnostic mutation tests fail when origin propagation is
broken, all current snapshots remain identical, and successful compilation
performs no redundant reachable-forest walk.

### P6 — Validate across semantic contexts and consumers

Run, at minimum:

- the reduced Jiles-Atherton scaling matrix;
- both full faustlibraries primary cases;
- `cargo test --workspace`;
- `compile-budget-check` and `compile-profile`;
- `examples-compare` over the 1,110 faustlibraries symbols;
- impulse/golden comparisons for scalar and vector code generation;
- CLI transcript and diagnostic snapshot checks;
- FAD, nested recursion, clock-domain, metadata, and grouped-UI tests;
- one compiler-library benchmark using the system allocator;
- one CLI benchmark using mimalloc.

For output equivalence, compare at least:

- dumped Signal IR;
- dumped FIR;
- generated C++;
- diagnostics and UI metadata;
- impulse output where applicable.

Measure peak RSS as well as time. A cache that buys time by retaining an
unbounded number of buses or context keys does not satisfy the objective.

**P6 exit criterion**: all semantic checks pass, both primary cases meet the
performance target, and the new normalized budget rejects a deliberate
reintroduction of the repeated work.

## 11. Recommended order of investigation

The shortest evidence-driven path is:

1. retain the five-point reduced scaling fixture;
2. add visit, bus, slot-lift, arity, context-cardinality, and provenance
   counters;
3. collect the matching C++ `FAUST_PROPAGATE_PROFILE` table for every reduced
   variant;
4. compare raw versus smoothed `Ms`, not merely slow versus fast DSPs;
5. simulate an exact canonical-key cache without retaining results;
6. introduce persistent `SlotEnvId` and `UiPathId`, then a non-allocating input
   bus key;
7. retest the C++-style exact result memo with all Rust-only semantic context
   represented or explicitly bypassed;
8. measure provenance separately after the core repeat factor is reduced;
9. land the normalized performance budget in the same change as the accepted
   optimization.

The critical discipline is now more precise: do not confuse a failed cache key
representation with a failed caching algorithm. The source-level differential
has already made the regression small enough to explain mechanically: one
recurrent parameter adds roughly 0.4 s twice, once in each independently written
DSP. The next implementation should account for that delta in operation counts,
then reproduce the cheap canonical identity that makes the measured C++ memo
profitable.

## 12. Conclusion

`ja_processor_stereo_ui_test` and `ja_transformer_demo_test` are high-value
compiler reference cases because they isolate a weakness that aggregate corpus
timings hide. faust-rs is faster than C++ Faust over the complete 1,110-case run,
yet it loses almost one second in propagation on this particular composition of
shared dynamic parameters and nested recursion.

The immediate task is not to simplify the DSP. It is to make faust-rs preserve
the sharing already present in the semantic graph, while retaining recursion,
UI, AD, clock, provenance, and diagnostic correctness. The plan deliberately
starts with retained differential evidence. It rejects the previously measured
non-canonical Rust memo key and the mechanical small-vector substitution, but it
now treats C++-style exact result memoization as the leading structural strategy
once slot environments, UI paths, propagation modes, and buses have cheap
canonical identities.
