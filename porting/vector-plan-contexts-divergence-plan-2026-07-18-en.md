# Vector Plan Contexts Divergence Plan

Date: 2026-07-18
Status: X1 complete (all three mechanisms traced; diagnostic corrected);
X2 complete (classes A and C fixed and qualified: 16-mode corpus sweep
unchanged at 97/34/1, byte-identity 180/182 with the two `echo_bug` drifts
characterized as common-subexpression sharing and oracle-verified 8/8, real
world directory 159 -> 165 vectorized); X2b complete (class B: P6.1 now
derives one-sample body history only for delayed recursive projections whose
alias and selected body have distinct P4.4 loop owners; lowering consumes that
accepted transition instead of inventing a current-sample transport, while
same-loop recursion and scalar preparation remain unchanged; `pm.ks` is
certified and bit-exact for both loop variants, all four strategies, and
vector sizes 32 and 24); X3 complete (the expanded 132-DSP corpus remains
97/34/1 in every one of the 16 modes; 192/194 pre-X2b FIR emissions are
byte-identical, and the two `gate_compressor` emissions have exactly identical
native output plus an 8/8 C++ oracle; WAC rises 165 -> 173 certified out of
197; compile-budget, optimized/unoptimized traces, and workspace gates pass)
Scope: agreement between the vector plan's execution-context model and what the
lowering materializes

## 1. Objective and baseline

At X1, the 93-DSP impulse corpus stood at 91/93 with only the E3
foreign-function class open. The corpus subsequently grew to 132 DSPs through
the clocked/multirate fixture expansion; X3 measures 97 certified, 34
fail-closed, and one error in every precision/loop/scheduling mode. Measured
against a real-world directory of 197 DSPs
(`WAC 2017/Faust`), the picture is different: 159 vectorize, 34 fall back, 4
fail to compile. **24 of the 34 fallbacks are one bug** under three
manifestations, none of which the corpus exercises.

The shared cause is the `contexts` map in `build_vector_plan`
(`vector/plan.rs`): the plan's model of *which loops execute which signal*.
Transports are derived from it, while the lowering materializes values by
walking the actual signal expression tree. Where the two disagree, each
direction of disagreement surfaces as a different error:

| Class | Divergence | Diagnostic | Files |
|---|---|---|---|
| A | context absent | `vector-plan dependency names missing signal N` | 11 |
| B | context too narrow | `no planned transport for signal N from loop O to loop C` | 8 |
| C | context too wide | `transport T for signal N load is absent from its consumer body` | 5 |

Class A covers the MIDI physical models (`clarinetMIDI*`, `guitarMIDI`,
`violinMIDI`, `elecGuitarMIDI`); class B the Karplus family (`karplus*`,
`chain`); class C the virtual-analog family (`virtualAnalogForBrowser`,
`WAWE`, `exp`, `minimoog-novation`).

The remaining 10 fallbacks are unrelated and already classified: 6 foreign
`ffunction` (the planned E3 class), plus one each of recursive-projection
assembly, event-bound overflow, undeclared module variable, and dependence
reversal.

## 2. Established mechanism - class A

Fully traced on `clarinetMIDI.dsp`:

- `contexts` is populated only inside `PlacementState::visit`, after three
  early returns: structural carriers, non-sample-required signals, and
  signals already placed `Control`. The insert sits *before* the `visited`
  guard, so any `visit` call on a signal creates its context; a signal with
  no context was therefore never visited.
- `visit` returns at a structural carrier **without recursing into its
  children**. The module doc states the opposite contract: "Symbolic
  recursion carriers and table containers remain structural: their executable
  children, rather than the containers, enter the sample closure."
- Independently, `Owned` placements are pre-seeded from the `owner` map
  before any traversal, and that loop does not touch `contexts`.
- `add_dependency_edges` then iterates over **every** certificate dependency,
  not only those reached by the traversal, and fails on the first missing
  context.

Measured: signal 2196 is `sample_required`, non-structural, in `owner`
(`Owned(8)`), and has a record - but no context. Its only two parents are
2202 (structural, so `visit` stops there) and 2205 (not itself reached).
Pruned traversal covers **256 of 305** signals; the raw child graph covers all
305. The 49-signal gap is the exposure.

The diagnostic is also misleading: the record is present, only the context is
missing, yet the message says "dependency names missing signal 2196". This
cost real diagnosis time and is fixed independently of the substance.

## 3. Established mechanisms - classes B and C (X1)

Both trace to symbolic multi-projection recursion groups, and together with A
they sharpen the unified statement: the certificate's dependency edges carry
three distinct semantics - value use, scheduling-only, and delayed - and the
plan flattens all three into one context/transport model, each wrongly.

**B - a `Delayed` edge nobody serves.** One-line reproducer
`import("stdfaust.lib"); process = pm.ks(200, 0.5);`. Consumer 235
(`SIGPROJ(0, SYMREF(W1))`, loop 1) depends `Delayed { amount: 1 }` on 1036
(`SIGPROJ(1, SYMREC(W2))`, loop 0, the group's value signal). The decoration
record of 1036 carries `max_delay = 0, delay_reads = 0`: **the certificate is
self-inconsistent** - a delayed scheduling edge targets a record with no delay
facts, because the delay facts and the dependency edges come from different
projections that disagree on cross-group symbolic reads. Consequently nobody
serves the read: transports are derived only from `delay == 0` occurrences
(plan.rs, `cross_uses` filter) and `DelayTransition`s only from
`max_delay > 0` records (state.rs). The lowering then resolves the raw value
cross-loop (`lower_raw -> resolve_in_loop`, backtraced) and fails
`MissingTransport`. A stdlib-free reduction was attempted (multi-projection
cross-group delayed reads, with and without state) and certifies; the
karplus termination/chain structure resists synthetic reduction, so the
one-liner above is the regression instance.

**X2b resolution.** The accepted design is vector-local transition coverage,
not preparation normalization. P6.1 combines the checked delayed dependency
with checked P4.4 placement: when the dependency source is a recursive
projection and source/target are owned by distinct loops, the selected body
receives an effective history bound at least equal to the delayed edge amount.
The independent P6.1 checker derives the same obligation from the decoration
certificate and vector plan. `lower_raw` reads that history only when both the
cross-loop placement and accepted transition are present; otherwise it keeps
the established same-loop lowering. This excludes the tempting but incorrect
current-sample transport and avoids duplicating state for explicit same-loop
`delay1`/lockstep recursions. No scalar signal tree or public API changes.

**C - a scheduling-only edge transported as a value.** Minimal stdlib-free
reproducer:

```
fA(x,y) = y + 0.125, x * 0.5;
gA = fA ~ (_,_);
aOut = gA : _,!;
process = attach(_, aOut);
```

Consumer 5551 in the WAC case is `SIGATTACH(x, SIGPROJ(0, SYMREC(W21)))` with
an `Immediate` edge to the projection. `attach(x, y)` returns `x` and only
forces `y`'s computation - a pure ordering edge. The plan flattens the
`Immediate` edge into `cross_uses` and plans a value transport; the
`SIGATTACH` lowering discards the attached value, so the consumer body never
loads it, and the body check rejects - correctly, which is why this class
fails closed. Negative results that narrowed it: not the E2 walker blindness
(a generic child enumeration does not find the load either), not a state-plan
consumption (no prefix, delay, recursion, or waveform transition consumes it).

## 4. Correction shape (informed by X1)

One phase, three edge semantics handled explicitly:

- **A - traversal closure**: `visit` must recurse through structural carriers
  to their executable children (the module's own stated contract), and
  pre-seeded `owner` placements must enter the traversal, so every signal a
  certificate dependency names carries a context.
- **B - state-plan consistency**: the occurrence and scheduling projections
  remain distinct, but a cross-loop recursive `Delayed { amount }` dependency
  must produce an effective P6.1 history obligation of at least `amount` for
  the selected body. The state producer and checker combine the checked
  dependency with P4.4 ownership; the lowerer consumes only that accepted
  transition. No current-sample transport is permitted for the back-edge.
- **C - scheduling-only edges**: `attach`-style forcing edges must produce
  ordering edges, never `cross_uses` transports. The dependency projection
  knows the source shape; the flattening point in `add_dependency_edges` is
  where the distinction dies today.

Anything that cannot be proven stays fail-closed with its diagnostic.

## 5. Independence obligation

The plan is a producer whose independent checker is `verify_vector_plan`, and
the divergence is precisely a fact neither currently checks. The new
obligation must be checkable from the plan and the emitted region bodies
alone:

- X.C1: for every transport in the plan, its consumer loop's emitted body
  contains a load of it (or an accepted state-transition consumption). This is
  today's body check, which already catches class C - it must stay and must
  not be weakened to admit the orphan.
- X.C2: for every cross-loop use the lowering resolves, the plan contains a
  matching transport. This is today's routing check, which already catches
  class B.
- X.C3 (new): every signal carrying a placement carries a context, and every
  context names a loop that exists in the plan. Class A is exactly the absence
  of this check - the failure surfaced by accident, deep inside dependency
  edge construction, rather than as a stated obligation.

X.C1 and X.C2 already exist and are what makes B and C fail closed rather than
miscompile. The phase must not relax them.

## 6. Rejecting mutations and focused tests

- M1: a forged plan whose signal has a placement but no context is rejected by
  X.C3 naming the signal.
- M2: a forged plan with a transport whose consumer body does not load it
  stays rejected (class C's current behaviour, pinned).
- M3: a forged plan missing a transport for a resolved cross-loop use stays
  rejected (class B's current behaviour, pinned).
- P1: a corpus-independent fixture reproducing class A - a sample-required
  signal reachable from the roots only through a symbolic recursion carrier -
  certified under both loop variants and all four strategies. It must fail by
  construction before the fix.
- P2, P3: the same for the B and C shapes, once X1 has reduced them to a
  minimal reproducer. If a class cannot be reduced, its corpus DSP is the
  regression instance and the journal says so.

## 7. Rollout

### X0 - plan

This document. No compiler behavior change.

### X1 - mechanism and diagnostics

Trace the B and C divergence paths to a line; correct the class A diagnostic,
which reports a missing record when the record exists and the context does
not. Add minimal reproducers. No behavior change beyond the message.

### X2 - invariant and fix

The chosen correction shape, X.C3, and M1-M3 with P1-P3. The widening and its
check land together.

### X2b - cross-group previous-sample state

> **Superseded 2026-10-29** by
> [`vector-symref-projection-edge-fix-plan-2026-10-29-en.md`](vector-symref-projection-edge-fix-plan-2026-10-29-en.md)
> (issue #24). The premise below is wrong: C++ `getSignalDependencies` makes
> `proj(i, rec) -> body_i` immediate, and the `~` feedback delay is the
> explicit `SIGDELAY1` above the back-reference. Serving the alias from its
> body's history therefore read it two samples late; `pm.ks` was
> miscompiled from frame 395 on, which the 64-frame bit-exactness test could
> not observe. The class-B inconsistency came from the analysis giving the
> back-reference edge `Delayed{1}`; that edge is now `Immediate`, the P6.1
> widening and the history read are removed, and `pm.ks` stays certified and
> is bit-exact over 1,024 frames.

Complete. P6.1 materializes checked history for cross-loop symbolic
pass-through back-edges and the pure lowerer consumes it conditionally. The
structural transition test pins the cross-loop placement predicate, the real
`pm.ks` regression is certified, and scalar/vector interpreter output is
bit-exact for `-lv 0/1`, `-ss 0..3`, and vector sizes 32 and 24. Same-loop
lockstep coverage remains unchanged and the full workspace gates pass.

### X3 - qualification

Complete. The full `f32/f64 x -lv 0/1 x -ss 0..3` sweep covers 132 DSPs and
reports exactly 97 certified, 34 fallback, and one error in each mode. No DSP
loses certification. The versioned baseline was refreshed to include the 39
clocked/multirate fixtures already present in the corpus; six of those are
universally certified, so the canonical list now contains 97 DSPs. X2b itself
does not newly certify a corpus DSP, hence there is no new-corpus-DSP C++
matrix to run.

Against `e8c49891`, 192 of the 194 `-lv 0/1` FIR emissions for those 97 DSPs
are byte-identical. The only differences are `gate_compressor` in the two loop
variants: P6.1 now materializes four cross-loop one-sample histories. Both
variants produce byte-identical 60,000-frame native output to the pre-X2b
compiler, and the current compiler passes the C++ reference for all eight
`-lv 0/1 x -ss 0..3` combinations.

The WAC directory moves from 165 to 173 certified out of 197, exactly closing
the eight class-B cases. The remainder is 20 fallbacks (11 assembly, six pure
foreign-function effects, two event-bound cases, and one cyclic plan) plus
four front-end errors. The scanner now supports checked deterministic shards
and a 64 MiB worker stack, allowing large library programs to be measured
without breaking their local imports or overflowing the default stack.

`vector-compile-budget-check` passes all five release cases, and the 40
optimized/unoptimized vector interpreter traces match. The complete retention
gate and workspace quality gates pass.

## 8. Risks and mitigations

- The external directory is not a committed corpus and cannot become a CI
  gate as it stands. X3 therefore treats its 173/197 result as evidence, not a
  gate. X2b's durable protection is the real `pm.ks(200, 0.5)` compiler
  integration regression, which exercises the previously absent Karplus
  cross-group back-edge under the full loop/scheduling matrix and two vector
  sizes without copying an external WAC artifact into the repository.
- Closing the traversal widens what the plan admits; every newly admitted
  program must pass the oracle, and already-certified changes require explicit
  qualification. X3 found only the two `gate_compressor` FIR shapes described
  above; exact native-output identity and the 8/8 C++ matrix rule out a runtime
  divergence despite the additional checked history storage.
- Class A's 49 unvisited signals in one DSP suggest the gap is broad rather
  than a single edge case; the fix must be measured by the visited/record
  ratio reaching parity, not only by the corpus turning green.
- If B and C prove to have distinct mechanisms after X1, the phase splits
  rather than forcing a unified fix onto unlike causes. The unified attribution
  is a hypothesis backed by shared symptoms, not yet a traced identity.
