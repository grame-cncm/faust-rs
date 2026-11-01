# Reporting several semantic errors in one compilation

Analysis and plan, 2026-11-01. Phase 1 is implemented, with the design of section 4, not the one first planned.

## 1. The question

faust-rs reports all the syntax errors of a program at once, and then stops at the first
semantic error. `process = ax + bx + c;` gives one error, for three undefined names. Each
further error costs a compilation. The logs of the blind repairs in
`faustprobe-diagnostics-eval` give 76 failed compilations for 118 errors over 20 programs,
which is why this was the first candidate after the work on the text of the messages.

## 2. What was measured

Two prototypes outside the compiler: after an undefined symbol the program is compiled again
with `x = _;` appended, until the failure is something else.

| Measure | Stop at first | All undefined symbols | Source |
|---|---:|---:|---|
| Simulated compiles, 248 scenarios | 3.30 | 3.26 | `simulate_multi.py` |
| Scenarios improved / worsened | | 9 / 1 | same |
| Programs with both typos found at once | 0 of 19 | 17 of 19 | replay of the blind programs |

The two disagree because of their mixes of errors. The simulator draws from nine kinds, and
two wrong names rarely meet: 46 further symbols over 1601 failed compiles. The blind programs
hold two typos each by construction, which is the favourable case.

The honest estimate is one compilation saved per program that holds two wrong names, or 17
of the 237 attempts of the blind repairs, 7 %. An earlier estimate of a sixth was wrong. The
value depends on how often real errors come in groups, which neither measure tells: a
first draft, a rename done by hand or a library upgrade do, a single slip does not.

The stages stay in order: syntax, then evaluation, then propagation. A program with errors of
all three kinds keeps three compilations, so failed compilations fall from 3.8 to about 2.9
per program at best, not to 1.

## 3. Options

**A. Static scope pass over the source.** Walk what `process` reaches and resolve each
identifier. Rejected: it restates the scope rules of `with`, `letrec`, patterns, `par(i, ...)`,
`environment`, `library` and `component`, and a difference becomes a false error. It also
walks code that lazy evaluation would never reach.

**B. Evaluate again.** The prototype's method: rerun evaluation with the symbol bound. Simple
and exact, but each run costs a whole evaluation on the error path. Rejected in favour of C,
which reaches the same errors in one run.

**C. Go on inside the run.** At the failed lookup in `eval_ident_value`, record the error and
return a wire. One pass, and no second evaluation. Rejected while implementing it: evaluation
tries things and drops their error (`a2sb(...).ok()` on arguments, `if let Ok(v)` on a constant,
`a2sb(...).unwrap_or(...)` in `route`), so a symbol met there does not fail the program today.
Recording it would refuse programs that compile.

**E. One further run after the failure (implemented).** The ordinary run stands. If it fails on
an undefined symbol, the program is evaluated once more with the recording of option C on. No
program can change verdict, and the first error is the same, by construction: the code that
decides runs unchanged. The price is one more evaluation on the error path, under a budget.

**D. Several composition mismatches.** After a mismatch, walk the flat box with its own arity
memo and collect the nodes whose children have an arity and which do not. A parent of a failed
node has no arity and stays silent, so no cascade is possible. It is independent of C. The blind
programs hold one arity error each, so its value is not measured.

## 4. Design of E

- `eval_entrypoint_full` keeps `eval_and_lower`. On an error, `with_further_undefined_symbols`
  acts on `UndefinedSymbol` only, and leaves any other error alone.
- The further run has a fresh `LoopDetector` with an `UndefinedCollection`. `eval_ident_value`
  records a failed lookup there and gives a wire. Without a collection it fails as it always did,
  so every other detector behaves as before.
- What the further run fails on is dropped: the wire may have caused it.
- The others go in `further: Box<[EvalError]>` of `UndefinedSymbol`, and
  `pipeline_to_boxes_step` builds one diagnostic per error, with a note on the further ones.

| Risk | Consequence | Mitigation |
|---|---|---|
| The further run is long | a failure that took a second takes minutes | Budget of twice the first run, at least 1 s, polled with the cancel flag. At most 8 errors. |
| A wire changes the path taken | a name in code the repaired program would not reach is reported | It is undefined all the same, and the note says how it was found. |
| A wire has one input and one output | `x(1, 2)` ends the search, as in 2 of the 19 programs | Accepted: what was found stays, and nothing false is reported. |

## 5. Plan

**Phase 1, option E: done, 2026-11-01.** Pass criteria, all met:

- `process = ax + bx + c;` gives three diagnostics, each located, in the order of evaluation.
- The first diagnostic of every failing program of `tests/corpus` is identical to today's.
- A program that compiles has an identical output: `golden-check`. The code that decides
  changed only at a failed lookup outside a further run, where it builds the same error.
- A test where a name is used in a dead `with` definition stays accepted.
- Mutation: with the deadline removed, a test on a long evaluation fails.
- `mutants.py` and `simulate.py` rerun with the real compiler. The gain must reach the
  prototype's, 3.26. `compile-budget-check` on CI.

Measured: `first_error_diff.py` runs the previous binary and this one on the 2991 mutants, and
gives the same verdict and the same first error for all of them. Simulated compiles go from
3.30 to 3.27. `compile-budget-check` has not run, as it cannot on this Mac.

**Phase 2, option D.** Only if phase 1 shows a gain worth it, and after a measure on programs
with two arity errors, which needs a new mutant kind first.

**Not planned.** Going on past a syntax error into evaluation: recovery makes up tree nodes,
and evaluating them would report errors that are not in the source.

Registry: a new line in DIFF-CLI-016, since faust-rs would report more errors than the C++
compiler, which stops at the first.

## 6. Recommendation

Phase 1 is cheap, about a hundred lines on an error path, and cannot change what is accepted.
Its gain is modest: 7 % of the attempts in the favourable case and under 2 % in the simulated
mix. It is worth doing for the user who gets several errors at once, not for the averages.
Phase 2 waits for evidence.
