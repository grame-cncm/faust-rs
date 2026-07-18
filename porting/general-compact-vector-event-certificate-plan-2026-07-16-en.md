# General Compact Vector Event Certificate Plan

Date: 2026-07-16
Status: implemented and validated (Phase C)
Scope: checked signal-level vector event certification for every routed plan

## 1. Objective and baseline

The P5.3/P6.1 event certificate currently expands every loop event for all
`vec_size` samples unless the verified vector plan contains a lockstep bundle.
At the default `-vs 32`, this finite expansion causes 26 of the 43 remaining
vector fallbacks (`FRS-VEC-FALLBACK-EVENTS`). `smoothdelay.dsp`, for example,
needs 4,967 expanded events while the production bound is 4,096.

Commit `a85da004` introduced a canonical two-sample repetition basis for
lockstep plans. Routed loop templates are already static per sample, so this
phase extends the same executable-certificate argument to every routed plan
whose producer and independent checker can establish sample translation
invariance. Complete evidence retains the 4,096-event limit, compact evidence
uses the separately approved 32,768-event limit, and the requested `vec_size`
is never reduced.

The review's target estimate was approximately 75 of the 93 DSPs in every
float/double, `-lv 0/1`, and `-ss 0..3` mode. The measured result is 62/93 in
all 16 modes. All 21 size-only event fallbacks pass the compact gate: 13 become
fully certified and eight reach a later fail-closed scheduling or lowering
diagnostic. The five remaining `FRS-VEC-FALLBACK-EVENTS` cases are genuine
`FissionSafe` reversals, not event-bound failures.

## 2. Canonical finite model

For a logical chunk length `N`, the routed plan supplies:

- fixed control definitions and effects;
- fixed epoch enter/exit events;
- fixed `LoopPre` and `LoopPost` state actions;
- for each loop `L`, one ordered per-sample template `T(L)` containing routed
  definitions, unmanaged effects, transport stores/loads, routed uses, and
  `LoopExec` state actions.

The expanded model materializes `T(L, n)` for every `0 <= n < N`. When that
model exceeds the event bound, the compact model materializes the fixed events
plus the canonical basis `T(L, 0)` and `T(L, 1)` for every loop. The first
sample covers every static operation and intra-sample dependency. The
`0 -> 1` boundary covers every adjacent recursion and managed-state carried
dependency. Translation invariance then applies the same obligations to every
`n -> n + 1` boundary in the logical chunk.

For `N < 2`, the complete model is retained. `sample_count` always records the
requested logical chunk length; `checked_sample_count` records the finite basis
length and is never used to alter FIR lowering or the physical chunk driver.
The complete form retains its 4,096-event limit. Following explicit user
approval after the first sweep, the compact form has a separately versioned
32,768-event limit. The largest measured basis is 28,843 events for f64
`reverb_designer.dsp`.

## 3. Producer obligations

The producer may select the compact basis only when the full independently
bounded event count exceeds the configured limit and the two-sample basis
fits. It must:

1. derive templates from the verified routed trace and checked state plan;
2. materialize every fixed event exactly once and every loop template exactly
   at samples zero and one;
3. build canonical scalar sample-major and vector epoch/loop-major orders over
   that basis;
4. include all intra-context, epoch, loop-edge, transport, use/definition,
   effect-resource, and managed-state dependencies;
5. include each recursion or state edge carried from sample zero to sample one;
6. preserve the logical `vec_size` in the certificate and in downstream FIR.

The producer must not use lockstep membership as an eligibility shortcut.
Lockstep and fused groups only affect the already-defined vector order.

## 4. Independent checker obligations

The checker must not consume producer grouping state, producer event totals, or
producer dependency summaries. From the vector plan, routed trace, and state
plan it independently:

1. reconstructs one-sample event keys and separates fixed from repeated keys;
2. computes both the full and two-sample bounds with checked arithmetic;
3. selects complete or compact evidence from those reconstructed counts;
4. reconstructs the exact finite event table and rejects missing, duplicate,
   reordered, or changed event keys;
5. compares each loop's sample-zero and sample-one template after removing only
   the sample coordinate;
6. rejects sample-index-dependent event kinds, resources, conditions, clock
   actions, or state actions that cannot be represented by one repeated
   template;
7. reconstructs scalar and vector orders and every required dependency;
8. verifies that each dependency agrees with scalar execution and remains
   ordered by vector execution (`FissionSafe`).

Clock islands are eligible only when their routed loop and checked state
actions are identical at samples zero and one. Chunk-entry/chunk-exit work must
remain in fixed `LoopPre`/`LoopPost` regions. Any non-uniform clock or state
shape is rejected with `CompactRepetitionMismatch`; it is not approximated.

## 5. Rejecting mutations and differential tests

Focused tests must reject at least these compact-certificate mutations:

- `checked_sample_count` changed from the canonical basis;
- a missing, duplicate, or changed sample-one template event;
- an event moved between sample zero, sample one, or a fixed region;
- a changed effect resource, transport identity, clock action, or state action;
- a missing carried recursion/state edge from sample zero to sample one;
- a scalar or vector order mutation.

Generated small fixtures must be checked twice: once with a high limit forcing
complete expansion and once with a low limit forcing compact evidence. Both
forms must agree on acceptance/rejection for pure transport, observable effect,
recursion/delay, fused-group, lockstep, and clock-island shapes. The literal
expanded model remains a test oracle only and is not used by the production
checker.

## 6. Rollout and result

### C0 — plan and baseline

- Record this model before implementation.
- Capture the 16-mode 49/93 coverage baseline and the exact 26 event-fallback
  DSPs.

Completed in commit `68abe4cd`.

### C1 — general compact producer/checker

- Remove lockstep-only eligibility from the route-independent precheck,
  producer basis selection, and checker basis reconstruction.
- Add explicit repetition-eligibility reconstruction and rejecting mutations.
- Add expanded/compact differential tests for representative routed plans.
- Keep the complete-evidence limit and all fallback reason codes unchanged;
  version any separately approved compact-evidence limit explicitly.

Completed with separate complete/compact limits after the first implementation
sweep proved that a shared 4,096 limit retained nine size-only fallbacks. The
error taxonomy and fail-closed behavior are unchanged.

### C2 — corpus qualification

- Require `smoothdelay.dsp` to retain certified vector FIR at default
  `-vs 32` for both loop variants and all four scheduling strategies.
- Run `count_vector_corpus --json` for all 16 modes and record every converted
  or still-failing DSP with its reason.
- Refresh `tests/vector-coverage/` and its universally certified benchmark
  list from those reports.
- Run the native C++ impulse oracle for every newly certified DSP: scalar
  `-ss 0..3` plus `-lv 0/1 x -ss 0..3`, 60,000 samples per response.

Completed for 13 newly certified DSPs, producing 156 successful native C++
comparisons. `smoothdelay.dsp` is certified in all 16 coverage modes. The
versioned baseline and universal benchmark list now contain 62 DSPs.
The exact retention gate recompiles all 992 certified mode/DSP pairs using at
most four isolated workers and reports results in deterministic mode order.

## 7. Acceptance gates

The phase is complete only when:

- formatting, warning-denied workspace Clippy, and all workspace tests pass;
- Rust golden output is byte-identical unless a separate parity-sensitive
  refresh is explicitly approved;
- all 16 vector-coverage modes pass with the refreshed checked baseline;
- `vector-interp-opt-check` passes;
- every newly certified DSP passes the native C++ impulse matrix;
- `smoothdelay.dsp` is certified without lowering `vec_size`;
- no unsupported non-repetitive plan is accepted;
- before/after coverage, event counts, and release compile timings are recorded
  in the daily journal.

## 8. Risks and mitigations

- **Unsound induction step:** require an exact sample-zero/sample-one template
  match and reconstruct all adjacent carried edges independently.
- **Hidden clock non-uniformity:** admit clocked plans only through identical
  routed and state templates; otherwise fail closed.
- **Certificate/checker common-mode bugs:** keep producer and checker count,
  table, order, and dependency reconstruction separate and compare both with
  expanded test oracles.
- **Accidental chunk shrink:** assert logical `sample_count == plan.vec_size`
  and retain existing chunk-driver coverage checks.
- **Resource growth after coverage expansion:** retain the 4,096 complete bound
  and the separately versioned 32,768 compact bound, then run the release
  compile-budget gate after qualification.
