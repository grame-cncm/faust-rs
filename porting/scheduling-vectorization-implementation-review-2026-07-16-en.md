# Scheduling and Vectorization Implementation Review

Date: 2026-07-16

Status: independent quality review of the implementation stream started at
`14376ddd` ("Implement P1 generic scheduler core"), up to `ce134678`
("Complete lockstep SIMD remediation gates")

Working branch: `ondemand-vec-fad-synthesis`

Related documents:

- [`vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md`](vector-mode-signal-level-analysis-cpp-port-plan-2026-07-10-en.md)
  (the master plan: P0–P8)
- [`lean-rust-certified-porting-plan-2026-07-11-en.md`](lean-rust-certified-porting-plan-2026-07-11-en.md)
  (the assurance plan: R0–R7)
- [`vector-corpus-coverage-analysis-and-recovery-plan-2026-07-15-en.md`](vector-corpus-coverage-analysis-and-recovery-plan-2026-07-15-en.md)
  (coverage recovery: Phases 0–6, complete)
- [`lockstep-simd-remediation-plan-2026-07-16-en.md`](lockstep-simd-remediation-plan-2026-07-16-en.md)
  (lockstep SIMD: Steps 0–4, complete)
- [`vector-mode-scheduling-formal-spec.lean`](vector-mode-scheduling-formal-spec.lean)

## 1. Executive summary

The stream comprises **74 commits over five days (2026-07-12 → 2026-07-16),
327 files changed, ~57,900 insertions**. It delivered, end to end: a generic
four-strategy scheduler (`-ss 0..3`) that is now the **authoritative
production scalar lowering order**; a complete signal-level vector pipeline
(`-vec -lv 0/1 × -ss 0..3`) built as a chain of producer/checker trust
boundaries (analysis → decoration certificate → vector plan → routing → pure
lowering → event certificate → state/clock/AD transitions → module assembly);
a lockstep instance-vectorization extension with verified native-SIMD
evidence; a 72-combination × 92-DSP executable backend matrix (6,624 oracle
comparisons, all passing); and permanent CI retention gates.

**Overall verdict: this is a high-quality, unusually well-evidenced
implementation.** The architecture is disciplined (fail-closed, opaque
verified types, independent checkers with rejecting-mutation tests), the
numerical validation is genuinely differential against the C++ oracle, and
the journal is honest — including a self-caught false SIMD claim that was
publicly corrected and remediated the same day.

The main findings of this review are:

1. **The workspace test gate is currently red.** One scalar scheduling test
   fails at HEAD; this review root-caused it to a stale test expectation
   exposed by the new scalar load-CSE pass (§5.1), not to a scheduling
   defect. It must be fixed — two commits were landed on a knowingly red
   gate.
2. **Vector coverage is 49/93 corpus DSPs (~53%)**, and the single dominant
   limiter is the bounded event-certificate budget (26 of the 43 fallbacks).
   The compact repeated-evidence mechanism that fixed this for lockstep
   bundles has not yet been generalized (§6.1).
3. The remaining gaps are all **known, named, and fail-closed** — nothing in
   the fallback set is silent or mislabeled — but several deferred items
   (user documentation, Lean L3 CI, C++ vector topology oracle, cost model)
   have no owner or scheduled phase (§6).
4. A code-level pass over the produced modules (§7) found idiomatic,
   deterministic, `unsafe`-free code with strong internal documentation, and
   three concrete performance findings: a missing verified-pair memo in the
   lockstep isomorphism traversal (path-count blow-up risk, same family as
   the fixed `compute_recursiveness` bug), duplicated O(E²) effect-conflict
   scans in the event certificate (both a compile-cost driver and coupled to
   the coverage limiter), and post-activation scaffolding (shadow report,
   PV slice, FIR `Drop` roots) still living on the production path.

## 2. What was reviewed

- The complete commit range `14376ddd~1..ce134678` (74 commits), read against
  the per-day journal entries (`porting/journal/2026-07-12.md` …
  `2026-07-16.md`), which document every commit.
- The plan status annotations inside the four related documents.
- The current code layout: `crates/transform/src/schedule/` (~2,500 lines
  including tests) and `crates/transform/src/signal_fir/vector/` (~24,200
  lines across 14 modules; largest: `assemble.rs` 3,290, `events.rs` 2,819,
  `analysis.rs` 2,768, `verify.rs` 2,650, `lower.rs` 2,573).
- The versioned coverage baseline `tests/vector-coverage/corpus-baseline.json`
  (16 modes × 93 DSPs) and the P7 report
  `porting/generated/p7-executable-backend-matrix-2026-07-14-en.md`.
- Live re-runs performed for this review: the failing test
  `recursive_apf_compute_body_reflects_all_four_cpp_schedules` (confirmed
  red), plus a differential build of the recursive-APF fixture at HEAD and at
  pre-load-CSE commit `01165f48` under all four `-ss` values to root-cause
  the failure (§5.1).

## 3. What was delivered, by layer

### 3.1 Generic scheduling (P0–P3, R1) — complete and production-active

- **P1** — literal ports of the four C++ strategies (`dfschedule`,
  `bfschedule`, `spschedule`, `rbschedule`) behind one `ScheduleDag` trait,
  with shared self-edge/cycle pre-validation (the C++ originals never check)
  and an independent `verify_schedule` mirroring the Lean `coversB`.
  Test depth is exceptional: hand-derived exact-order snapshots
  cross-checked against the Lean `diamondGraph` fixture, plus **exhaustive
  enumeration of all upper-triangular DAGs for n = 1..6 (33,867 graphs × 4
  strategies)**.
- **P2** — `-ss` CLI/builder/FFI plumbing with strict parsing (documented
  divergence from the C++ `atoi` fallback) and canonicalized cache identity
  (`-ss 3` ≡ `-ss 42`).
- **P0** — a 13-case structural corpus with 151 captured C++ artifacts from
  the pinned reference `8eebea429`, which surfaced two load-bearing oracle
  facts: the pinned branch **rejects `-vec` wholesale** (so no C++
  vector-topology oracle exists — see §6.6), and `-ss` changes scalar C++
  byte-wise (giving P3 its differential target).
- **P3** — activated in three carefully staged steps: structural changes
  proven behavior-preserving by golden-check; a **shadow mode** whose
  recorded finding (zero immediate-edge inversions, but only 5/9 programs
  already matching the DFS schedule) served as the explicit go/no-go input;
  then the authoritative flip, validated by `make all-ss` (92 DSPs × 6
  backends × 4 strategies = 2,208 scalar comparisons). Activation exposed
  and fixed exponential growth in the literal `spschedule` port (memoized,
  with the literal form kept as a test oracle) and, later, a real
  regression (sibling clock-region temp leakage) that was fixed with a
  region-stacked lowering cache.
- **R1** — `ScheduleCertificate` with hand-written canonical byte encoding
  and SHA-256 `graph_hash`, one rejecting mutation per error variant.

### 3.2 Signal-level vector pipeline (P4–P6) — complete for the supported subset

Nine producer/checker boundaries, each additive when landed and each carrying
focused mutation tests:

- **P4.1–P4.3a** — one unified signal-use walk (single decoder for
  scheduling and occurrence projections, the C++ `OccMarkup` rules,
  `hi + 0.5` delay rounding), canonical positive-DNF execution conditions,
  stable effect-resource identities, conservative transitive effects.
- **P4.3b** — schema-versioned `DecorationCertificate` +
  `verify_decorations`, stricter than `SigType::PartialEq`.
- **P4.4** — `build_vector_plan`, whose only semantic input is the opaque
  verified certificate; ports the C++ `needSeparateLoop` precedence at the
  decoration boundary; freezes loops/epochs/placements/transports **before**
  `-ss` can order anything (the strategy-independence invariant).
- **P5.1–P5.3** — `VectorRouteSession` (region-scoped value caches,
  pre-planned transports only, no on-demand allocation), pure lowering with
  per-region CSE, and the bounded **FissionSafe event certificate**, whose
  tests demonstrate the key counterexample (two effectful loops reversing a
  cross-sample dependence even with a static effect edge).
- **P6.1–P6.6** — delay state with C++ copy/ring geometry validated by
  `DelaySim` over 32,805 bounded input sequences; clock islands (OD/US/DS)
  with executable `ClockStep` evidence; recursion tuples routed through
  scalar projections; module assembly with lifecycle checking; clock-local
  state (one ring + shared fire-time cursor per domain) and bounded
  variable delays. Reverse AD retains an explicit `FRS-VEC-RAD-SCALAR`
  fallback policy with fixed forward/reverse epochs.

### 3.3 Coverage recovery (Phases 0–6) — complete; 3 → 49 certified

The fail-closed policy correction in `b5a0a8b3` (rejections rebuild as
scalar, not as the transitional vector builder) had dropped effective
coverage to 3/93. Six phases recovered it: actionable diagnostics
(`VectorPipelineStatus` + reason codes + JSON corpus counter), UI lifecycle
integration, plan admissibility (fused serial groups for recursive-delay
crossings, incl. a genuine three-loop indirect-path defect found by native
comparison and closed with reachability checking), state-transition
extensions (prefix, waveforms, `fconstant`, read-only tables, symbolic
carriers, state-mediated variable delays), retention gates
(`vector-coverage-check` recompiling all 784 claimed pairs in CI, plus a
16-cell precision × `-lv` × `-ss` matrix), and a scalar-cost audit that both
exonerated vectorization for the `reverb_designer` frontend cost
(evaluation-bound before either feature) and removed real overhead (scalar
effect analysis 140 ms → 2 ms).

Every newly certified DSP was validated against the **native C++ impulse
oracle over 60,000 samples across the full 12-configuration matrix** (48 to
204 comparisons per phase).

### 3.4 Lockstep instance vectorization (§8 + remediation) — complete

Lean-mechanized first (`Shape` isomorphism, `iso_decorations_agree` proved,
`LockstepSafe` = `FissionSafe`), driven by a measured benchmark (3.69×
bit-exact via plain fusion on 4 biquads), then implemented: schema-v2 trust
boundary, deterministic detector (pairwise incomparability + effect
commutation + constructor-for-constructor shape confirmation), bundle-as-one
scheduler node, single physical sample loop preserving per-lane IEEE order.

The remediation episode deserves emphasis: the first native-SIMD gate
measured **scalar fallback output** (the corpus case exceeded the event
budget) and reported 14 four-wide operations as lockstep evidence. The error
was caught in-house, publicly corrected in the journal, and fixed by a
five-step plan: gate now requires `VectorPipelineStatus::Certified` before
generating C++, compact two-sample event evidence for lockstep plans,
register-carried delay-one state, and **line-attributed LLVM evidence**
(vector IR must belong to the lockstep source region; scalar fallback is a
rejected negative test).

### 3.5 Backend matrix and benchmarks (P7.1–P7.2) — complete

All 72 backend/mode/strategy combinations over 92 impulse DSPs: 6,624
responses accepted against the C++ oracle, with a versioned report binding
byte counts and a SHA-256 aggregate. The run itself caught a real FIR shape
bug (bare `StoreTable` bodies in vector copy/clear loops). `make vec-bench`
provides the 12-combination scalar/vector × strategy throughput matrix.

### 3.6 Genuine pre-existing bugs found and fixed along the way

The stream's validation pressure surfaced six real defects unrelated to its
own new code: the silent-passthrough `table_state_delay` fixture; exponential
recursiveness memoization (`jprev_demo` ~2 GB / near-hang → ~5 s); the
`--double` parser precision-variant regression (`ma.MIN` → `inf`); a missing
waveform-element clock annotation; a P4.4 panic on inline-then-root output
signals; and the `pulse_countup_loop` stale-state vector transport (which
motivated the fail-closed policy). This is what a working assurance
methodology looks like in practice.

## 4. Quality assessment

### 4.1 Strengths

- **Trust-boundary architecture, consistently applied.** Every stage
  consumes an opaque `Verified*` artifact and re-derives its own facts;
  checkers never call producers; every checker has at least one rejecting
  mutation per error variant (the plan's §8 policy is actually enforced, not
  aspirational). Fail-closed is real: all 43 corpus fallbacks carry stable
  reason codes and rebuild as scalar; the Phase 0 sweep verified no
  mislabeled effective mode.
- **Differential validation depth.** Bit-exact interpreter cross-checks
  (scalar vs vector, both loop variants, all four strategies, non-dividing
  chunk tails), the 6,624-response backend matrix, 60,000-sample native C++
  oracle runs for every coverage claim, optimized/unoptimized parity traces,
  and golden snapshots kept byte-identical through every additive phase.
- **Bit-exactness treated as a hard contract.** The lockstep design
  explicitly rejects reassociating parallel-form IIR restructurings; the
  V4 NEON benchmark's 1e-5 drift was used to derive the FMA-contraction
  policy; `-ffp-contract=off` gates the SIMD evidence.
- **Formal layer that pays its way.** The Lean spec is not decoration: the
  duplicate-node gap in `verify_schedule` was caught against `coversB`; the
  lockstep legality obligations were mechanized before implementation;
  exact-order snapshots are cross-checked against Lean fixtures.
- **Performance regressions are caught in-flight.** Compile-cost profiling
  is part of the working loop (route double-verification −2.76 s, effect
  fixed-point rewrites 73.5 → 6.9 s on `bells`, event-count preflight
  29.0 → 20.7 s on `spectral_level`), and claims are measured, never
  asserted.
- **Honest reporting.** Interrupted validation runs are explicitly "not
  claimed"; the invalid SIMD numbers are struck through in place with a
  correction note; the red workspace test is named in the final commit's
  journal entry rather than hidden.

### 4.2 Weaknesses

- **A red gate was tolerated.** The last two commits landed with a known
  failing workspace test (§5.1). The journal documents it, but the
  repository's own rule is that the workspace gate passes before merge;
  "documented red" is still red.
- **Scale and duplication risk.** ~24k lines in `signal_fir/vector/` with
  five files above 2,500 lines. Independent recomputation is the assurance
  design, but `analysis.rs`/`verify.rs`/`events.rs` now each contain a
  variant of dependency/effect reconstruction; a shared-vocabulary refactor
  (without merging producer and checker) will eventually be needed to keep
  the checkers reviewable — their reviewability is the whole point.
- **Compile-time cost of certification is visible but unbudgeted.** The
  vector plan chain adds ~5.8 s on `reverb_designer` (on top of a ~60 s
  pre-existing evaluation-bound frontend). Individual regressions were
  fixed when noticed, but there is no CI budget/threshold that would catch
  the next one automatically.
- **Documentation drift.** Doc comments in `signal_fir/mod.rs` (lines
  ~180/210/256/317) still describe fallbacks as retaining "the transitional
  vector builder", contradicting the post-`b5a0a8b3` scalar-fallback policy
  those same commits implemented. The certified-plan header still says
  "production P4/P5 integration is not complete (2026-07-13)" although
  P4–P6 landed on 07-14. Minor, but these are exactly the documents a
  future implementer will trust.
- **User-facing documentation never landed.** The P2 verifier decision
  deferred documenting `-ss` (and the vector-mode `-ss 0` ≠ C++ `sortGraph`
  divergence) "to P5 activation". Activation happened; `README.md` still
  contains nothing about `-ss`, `-vec`, `-lv`, or `-vs`. The options are
  live and user-visible.

## 5. Defects found by this review

### 5.1 Red workspace test: `recursive_apf_compute_body_reflects_all_four_cpp_schedules`

`crates/compiler/tests/p3_shadow_mode.rs:197` asserts that the four `-ss`
strategies produce four distinct C++ `compute` bodies for a recursive APF
fixture. At HEAD it observes three. Root cause, established by rebuilding the
fixture at HEAD and at pre-load-CSE commit `01165f48`:

- Before the state-aware load-CSE pass, `-ss 0` and `-ss 1` differed **only**
  in the position of the `fRec196[0] = …` statement relative to a second,
  redundant materialization `float fTemp1 = fRec196[1];`.
- Phase C/D of the load-CSE plan (commits `10c86026`…`8ed16955`) correctly
  removes that redundant load (rewriting its uses to `fTemp0`, including the
  history store `fRec196[2] = fTemp0`). With the distinguishing statement
  gone, the two schedules collapse to the same emitted text.

So the generated code is correct and bit-exact; the *scheduling* is still
strategy-dependent internally; the fixture simply no longer has enough
independent statements to keep DFS and BFS textually distinct after CSE.
**Required fix:** either enrich the fixture (a second independent recursion
or a longer expression chain restores four distinct orders) or weaken the
assertion to "≥ 3 distinct and pairwise-diffable schedules with unchanged
semantics". Enriching the fixture is preferable — the test's purpose is to
prove `-ss` remains observable, and a fixture that barely clears the bar
will rot again. Until fixed, the workspace gate is red and every subsequent
"fully green" claim is qualified.

### 5.2 Process finding

Commits `c7db2ee8` and `ce134678` were landed on that known-red gate. The
journal is transparent about it, but this breaks the branch's otherwise
strict gate discipline and sets a precedent worth explicitly rejecting: a
pre-existing failure should be fixed or the test explicitly quarantined
(`#[ignore]` with a tracking note) *in the same commit series*, not narrated.

### 5.3 Stale documentation (see §4.2)

`signal_fir/mod.rs` transitional-builder doc comments; certified-plan status
header; missing README coverage for `-ss`/`-vec`/`-lv`/`-vs`.

## 6. Remaining work, prioritized

### 6.1 Generalize compact event evidence — highest coverage leverage

26 of 43 fallbacks (`FRS-VEC-FALLBACK-EVENTS`: `bells`, `freeverb`,
`reverb_designer`, `smoothdelay` at 4,967 events vs the 4,096 limit,
`spectral_level`, `tapiir`, …) fail only because the bounded FissionSafe
certificate expands the full chunk and exceeds its budget at default
`-vs 32`. The two-sample template + `n → n+1` boundary basis implemented for
lockstep bundles (`a85da004`) is exactly the needed mechanism; it currently
applies **only** when the plan contains a lockstep bundle. Extending the
canonical-basis argument to general routed plans (per-loop event templates
are already sample-repetitive by construction) would convert most of these
26 DSPs and is the single biggest coverage win available.

### 6.2 Generalize fused serial groups

13 fallbacks (`FRS-VEC-FALLBACK-PLAN`, `UnfusedImmediateDelayCrossing`:
`delays`, `echo_bug`, `zita_rev1`, `pitch_shifter`, …) are delay crossings
the fused-group certificate does not yet cover: multi-carrier groups, longer
chains, and clock-crossing shapes were explicitly excluded by the 07-15
fusion slice. The producer/checker skeleton (internal-transport
rematerialization, reachability closure) already exists.

### 6.3 Effectful tables, soundfile, RAD

- `math.dsp`, `table1.dsp`, `table2.dsp` (`FRS-VEC-FALLBACK-PURE`): mutable
  `rdtable`/`wrtable` writes remain fail-closed; needs a table-write effect
  resource in the state plan plus serial co-location, analogous to the
  existing UI-write handling.
- `sound.dsp` (`FRS-VEC-FALLBACK-UI`): soundfile data reads are not yet
  certified.
- Reverse AD keeps the `FRS-VEC-RAD-SCALAR` policy. Fine as policy, but the
  fixed Forward < Reverse epoch model already in P6.2 suggests block-level
  vector RAD is reachable; relevant to the P5/S4 differentiable-STFT goal
  in the ondemand/FAD roadmap.
- `subcontainer1.dsp` is an independent SIGGEN gap (`FRS-SFIR-0004`, foreign
  functions in the SIGGEN interpreter), not a vector issue — but it caps the
  corpus at 92 everywhere.

### 6.4 P7 remainder

- **P7.3**: FIR/WAST/Julia artifact matrix; single-precision impulse
  coverage where runners support it (the 16-cell coverage matrix counts
  certification but does not execute f32 impulse comparisons).
- Final-state/effect parity beyond the first impulse (multiple blocks,
  tables, UI zones) as an explicit gate.
- The **cost model**: today every plan-admissible split is taken. The plan's
  own measurements (0.92× for a simple multiply tail) show systematic
  separation is not always profitable; `vec-bench` exists but no decision
  procedure consumes it. Also the deliberate `-ss 0` vector-default
  divergence from C++ `sortGraph` should eventually be justified by
  benchmark, or the default reconsidered.
- Transitional-path removal: the transitional vector builder is no longer
  reachable from fallbacks, but its code and stale doc references remain.

### 6.5 Assurance-plan remainder (R2–R7)

RV, R1, R2D, R3(L2), and partial R4 are done. Still open: canonical JSON
export with RFC 8785 conformance and `plan_hash` (R2); the executable Lean
checker consuming exported certificates in CI (L3, R2/R3); routed-FIR
certificate completion (R4); the semantic reference executor (R5); selected
L4 refinement (R6); backend refinement gates (R7). These are the difference
between "independently checked in Rust" and "checked against the normative
Lean semantics" — currently all cross-language guarantees rest on the Rust
checkers plus differential testing.

### 6.6 Known parity and infrastructure gaps

- **No C++ vector-topology oracle**: the pinned reference rejects `-vec`
  wholesale, so vector-mode loop topology is validated only by Rust
  invariants + numeric equivalence. Pinning a separate mainline C++ build
  for topology comparison remains an unmade decision.
- **Per-clock-context delay lines** (07-15 note): a hash-consed startup
  delay line is shared between sibling clock domains where C++ allocates one
  per context; needs contextual (signal, domain) keys in the delay
  plan/manager. Runtime-parity issue, currently latent.
- `box-ffi`/`interp-ffi`/`wasm-ffi` do not thread `-vec`/`-ss` (pre-existing
  pattern, recorded honestly at P2, still true).
- Lockstep: longer-delay bundles need the planned SoA `state[delay][lane]`
  layout (remediation plan §5.3); near-isomorphic bodies (e.g. add/sub
  pairs) are correctly rejected today but could be normalized in future;
  Julia has no impulse runner, so its lockstep evidence is structural only.
- The 26 event-bound DSPs also mean the throughput benchmark's 49-file
  intersection under-represents heavy programs — worth remembering when
  reading `vec-bench` aggregates.

## 7. Code-level review

This section reviews the produced code itself — layout, idiom, robustness,
algorithmic behavior, and the quality of the generated output — based on a
direct reading of `crates/transform/src/schedule/` (~2,500 lines including
co-located tests) and `crates/transform/src/signal_fir/vector/` (~24,200
lines, of which roughly a third is co-located tests), plus the scalar-side
files this stream touched (`signal_fir/cse.rs`, `signal_fir/module/
core_lowering.rs`, `signal_fir/module/build.rs`, `hgraph/`).

### 7.1 Layout, style, and robustness — very good

- **`schedule/` is exemplary.** One strategy per file, each a short literal
  port with C++ provenance in the module doc; the shared pre-validation and
  the independent verifier live in their own files; the exponential literal
  `spschedule` is retained under `#[cfg(test)]` as an executable oracle for
  the memoized production form (`special.rs`), whose compositional
  `(logical length, node → last position)` summary is clearly explained,
  uses `u128` logical positions, and documents its DFS fallback beyond the
  parity domain. This is how an algorithm port should look.
- **Function granularity is healthy.** An initial scan suggested giant
  functions, but the spans are impl blocks: `lower_vector_program_impl` is
  ~215 lines of linear, stage-traced pipeline; the ~1,500-line span after it
  is the `PureVectorLowerer` impl containing ~50 focused per-node methods.
  `cse.rs` is decomposed into small single-purpose functions
  (`table_locations_may_alias`, `has_later_stack_store`, …). The largest
  genuinely monolithic function is `verify_vector_plan` (~650 lines), which
  is defensible for a checker: a flat, ordered sequence of certificate
  obligations that mirrors the schema, each with a typed error.
- **Zero `unsafe`, zero `TODO`/`FIXME`** across both module trees. The only
  clippy allowance in the vector tree is `too_many_arguments` (×10), an
  honest symptom of the artifact-passing style (functions receiving
  `prepared + plan + state_plan + clock_plan + ui + strategy + …`); a small
  pipeline-context struct would remove all ten.
- **Error handling matches the fail-closed philosophy.** Thirteen typed
  error enums with `Display` impls; fallbacks carry stable reason codes plus
  a human diagnostic string. Production `expect()`s are reserved for
  internal invariants with descriptive messages ("effect parent has a signal
  record"); acceptable, though each is an implicit panic path in a compiler
  that otherwise never panics — converting the handful on hot paths to
  internal error variants would be more consistent.
- **In-code documentation is unusually good.** Module headers cite the C++
  source files and plan sections; non-obvious code explains *why* (e.g. the
  effect-propagation rewrite documents the former
  `O(depth × signals × effects)` behavior it replaced; `events.rs` states
  its formal boundary). Comment density is uniform across the 74 commits —
  the multi-agent production process did not produce stylistic drift.

### 7.2 Determinism-first data structures — deliberate, and the right call

The vector tree uses 233 `BTreeMap` / 174 `BTreeSet` against only ~66
hash-based maps. This is a conscious design: certificates require strictly
ascending canonical orders (which `verify_vector_plan` actually enforces —
noncanonical-but-equivalent sets are rejected), stable names must be
`-ss`-independent, and iteration order must never leak into emitted FIR.
Where ordering is irrelevant, faster structures are used (`ahash` in the
special scheduler's summaries), showing the tradeoff is applied case by
case, not by habit. The log-factor and pointer-chasing cost is invisible at
current corpus scale; if vector-plan cost ever matters, dense `Vec`-indexed
tables keyed by the (already contiguous) loop/epoch ids with an explicit
sort at the certificate boundary would preserve canonicity at lower cost.

### 7.3 Algorithmic findings (compile-time)

Ordered by importance:

1. **Missing verified-pair memo in the lockstep isomorphism traversal**
   (`lockstep.rs`, `ParallelShape::visit`). The `active` set is a pure cycle
   guard (insert on entry, remove on exit); confirmed `(representative,
   lane)` pairs are never cached, so shared sub-DAGs are re-traversed once
   per path. On hash-consed signal graphs this is path-count complexity —
   exactly the class of blow-up fixed in `compute_recursiveness`
   (`b5944700`, `2^18` environments → linear). It is bounded today only
   because candidate lane bodies are small; a wide, heavily shared recursive
   bank could make `-vec` compiles explode. The fix is one memo set and is
   trivially sound (the verdict depends only on the pair).
2. **The bounded event certificate does two O(E²) pairwise effect scans**
   (`events.rs:1089` in the producer, `events.rs:1794` in the independent
   checker — the duplication itself is the trust-boundary design). E is
   effect events ≈ effects × `vec_size`, capped at 4,096, so up to ~8.4M
   `effects_conflict` calls per side. Combined with full-chunk expansion,
   this is simultaneously the coverage limiter (§6.1, 26 DSPs) and a
   measured compile-cost driver (`spectral_level` ~20.7 s even after the
   preflight bound). Generalizing the compact two-sample basis fixes both at
   once; short of that, grouping effect events by resource before pairing —
   the same restructuring already applied to scalar effect orientation in
   `7f510fc0` (140 ms → 2 ms) — removes the quadratic factor.
3. **Post-activation scaffolding still runs on the production path.** The
   shadow report (`signal_fir/mod.rs:727`) is recomputed for every compile
   as a "post-activation conformance trace" — O(nodes+edges), cheap but pure
   overhead now that activation is done; it belongs behind a debug flag or
   in a test. Likewise `pv_slice.rs` (671 lines) and the transitional vector
   builder survive as live `pub mod`s; P7's transitional-path-removal item
   should reclaim them (keeping their DSP cases as regressions, per plan).
4. **Minor allocation nits**: `propagate_effect_sets` clones the child's
   whole effect set on every worklist pop (a split-borrow or take/put
   avoids it); the lockstep `ShapeHasher` hashes `format!`-allocated string
   tokens per node where a discriminant byte + varint stream would be
   allocation-free. Both are second-order.

Credit where due: the stream repeatedly found and fixed its own complexity
regressions in-flight (route double-verification −2.76 s; whole-map effect
fixed point → changed-node propagation, `bells` 73.5 → 6.9 s; per-conflict
BFS → compact closures; event-count preflight, `spectral_level` 29.0 →
20.7 s; scalar effect orientation 140 ms → 2 ms), and the code carries
comments explaining each rewrite. What is still missing is a **CI
compile-cost budget** so the *next* regression is caught by a gate instead
of by someone profiling `reverb_designer` again.

### 7.4 Generated-code quality (runtime)

- The scalar load-CSE pass and the shared pure-`Drop` elision measurably
  improved emitted code (duplicate state-history loads gone, dead
  `(void)(…)` statements gone across C/C++/AssemblyScript/Julia, no-op
  drops skipped in interp/Cranelift/Wasm), all validated against the
  impulse oracle under all four strategies.
- The `cse.rs` alias analysis has the right safety asymmetry, documented
  and tested: a missed alias only loses an optimization; dynamic indices
  never enter the cache; calls and tees are barriers.
- Native SIMD evidence for lockstep is now genuinely trustworthy
  (certified-status precondition, line-attributed LLVM IR, scalar fallback
  as a rejected negative test).
- Remaining runtime-code improvements: the checked vector lowerer still
  materializes `Drop` scaffolding roots in FIR that every backend must then
  elide — sweeping them out of the FIR after verification would simplify
  all seven emitters at once; lockstep state beyond delay-one needs the
  planned SoA layout to vectorize; and with no cost model, every admissible
  fission is taken, so chunk transports can be emitted where inlining wins
  (the plan's own 0.92× measurement).

### 7.5 Maintainability

- **Independent-checker duplication is the design, but it is growing.**
  `analysis.rs`, `events.rs`, and `verify.rs` each contain a variant of
  dependency/effect reconstruction. Independence forbids sharing *state*,
  not *vocabulary*: extracting more small pure total functions (in the
  spirit of `effects_conflict`) into a common module both sides call would
  shrink the review surface without weakening the trust boundary — the
  checkers' reviewability is their entire value.
- The compatibility alias layer (`vector_analysis` → `vector::analysis`
  after the `d5681122` regroup, plus retained historical names like
  `VerifiedPureVectorProgram`) doubles the nominal API surface; one rename
  pass should be scheduled once the pipeline shape settles.
- Test co-location keeps producer, checker, and their mutation tests in one
  file — good for review, but it is what pushes files to 2,600–3,300 lines;
  moving only the test halves to `tests/` submodules (as `schedule/` already
  does) would halve the visual weight without touching structure.

## 8. Conclusion

For five days of work, the stream is remarkable both in scope and in rigor:
a production `-ss` scheduler validated by 2,208 scalar comparisons, a
certified vector pipeline validated by 6,624 backend-matrix responses and
per-DSP 60,000-sample oracles, and a lockstep extension whose performance
claims survived their own audit. The producer/checker discipline held under
schedule pressure — every cut corner is a *named*, fail-closed fallback
rather than a silent approximation, which is precisely what makes the
remaining work list in §6 credible.

The code itself (§7) reads like a single author's: deterministic
data-structure discipline, typed errors, zero `unsafe`, literal C++ ports
kept as test oracles for their optimized replacements, and comments that
explain complexity decisions. Its weaknesses are the predictable ones of a
five-day, 58k-line sprint: scaffolding left live on the hot path, one
missing memo with blow-up potential, and checker duplication that will need
a shared-vocabulary refactor to stay reviewable.

The immediate obligations are small and sharp: fix the red APF scheduling
test (§5.1), add the lockstep verified-pair memo (§7.3.1), refresh the
stale doc comments and plan headers, and document the now-active
user-facing options. The strategic next step is unambiguous: generalize
the compact event certificate (§6.1/§7.3.2), which alone stands between
the current 49/93 coverage and roughly 75/93 — and removes the O(E²)
compile cost — then the fused-group generalization (§6.2) for most of the
rest.
