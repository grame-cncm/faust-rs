# Compile-time analysis — where faust-rs spends 3.8× the reference's time

**Date**: 2026-08-06
**Status**: **P0 and P1 implemented 2026-08-06** — the corpus went from 3.81x
to 2.30x the reference's compile time. **P2 profiled and its hypothesis
refuted**; the remaining cost is library lexing, not memoization. **P2′
measured**: the lexer is 373x slower than a hand-written scanner; its
definition is now built once (2.30x -> 2.13x), replacing it is scoped but not
started. P3-P4 proposed and not implemented
**Trigger**: `make compile-bench` reports faust-rs slower than C++ Faust on 91 of
94 corpus DSPs; the question was why.
**Related**: `porting/cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md`
(a different, smaller cost — see §7), `porting/MEMOIZATION.md`.

---

## 1. The measurement

`make compile-bench` compiles every gated DSP with `-lang cpp -double` through
both compilers and records wall time.

| | C++ Faust | faust-rs | ratio |
|---|---|---|---|
| corpus total (94 DSPs) | 4.75 s | 18.11 s | **3.81×** |
| median per-DSP delta | — | — | **+122 %** |
| faster / slower / failed | — | 3 / 91 / 0 | — |

Worst absolute cases:

| DSP | C++ | faust-rs | delta |
|---|---|---|---|
| `reverb_designer` | 0.871 s | 7.226 s | +729 % |
| `phaser_flanger` | 0.337 s | 1.516 s | +350 % |
| `modulations` | 0.092 s | 1.026 s | +1014 % |
| `spectral_level` | 0.498 s | 0.981 s | +97 % |

Splitting the corpus by source size does *not* explain it: sources over 20 KB
run at 4.13× and ordinary sources at 3.78×. The cost is not a large-input
effect.

### 1.1 Where the time goes

Per-stage totals from `faust-rs -time`, summed over the same 94 DSPs
(18.03 s measured, matching the benchmark's 18.11 s):

| stage | seconds | share |
|---|---|---|
| **evaluation** | 12.064 | **66.9 %** |
| parser | 2.841 | 15.8 % |
| signal-fir | 2.522 | 14.0 % |
| — of which `fir-prepare-normalize` | 1.885 | 10.5 % |
| propagation | 0.401 | 2.2 % |
| fir-clock-analysis | 0.213 | 1.2 % |
| fir-lowering | 0.203 | 1.1 % |
| everything else (7 stages) | 0.197 | 1.1 % |

**The signal → FIR → codegen pipeline is not the problem.** It is 14 % of the
total, and `cpp-codegen` itself is 0.2 %. On the worst case, `reverb_designer`,
evaluation is 6.93 s of 7.20 s — 96 % — while the entire FIR side is 0.19 s.
This is worth stating plainly because that pipeline is where most recent effort
has gone; none of it is what makes the compiler slow.

---

## 2. The cause

### 2.1 What the profile shows

A `sample(1)` capture of `reverb_designer` (7 s, ~1537 samples, single hot
thread) puts every sample under `eval`. The recursion is

```
eval_value → apply_value_list_value → (deep self-recursion)
           → box_simplification → numeric_box_simplification
           → propagate_box_and_simplify → normalize::simplify::sig_map
```

Symbol frequencies across the captured graph: `simplify` 5942, `eval_value`
1128, `apply` 912. `box_simplification` and `propagate_box_and_simplify` carry
the largest single-frame sample counts.

### 2.2 The mechanism

`box_simplification` is memoized — but the cache is created by its caller, and
the dominant caller creates a fresh one **on every pattern-match dispatch**
(`crates/eval/src/apply.rs:168`):

```rust
let arg = {
    let mut cache = ahash::HashMap::with_hasher(ahash::RandomState::new());
    box_simplification(arena, &mut cache, raw_arg)
};
```

So the memo's lifetime is one argument. Every dispatch re-simplifies its whole
box subtree from scratch, and `apply_value_list_value` recurses deeply through
library code, so the same subtrees are re-walked at every level.

The reference does the opposite. `compiler/evaluate/eval.cpp:1603`:

```cpp
static Tree boxSimplification(Tree box)
{
    Tree simplified;
    if (gGlobal->gSimplifiedBoxProperty->get(box, simplified)) {
        return simplified;
    } else {
        simplified = numericBoxSimplification(box);
        ...
        gGlobal->gSimplifiedBoxProperty->set(box, simplified);
        return simplified;
    }
}
```

`gSimplifiedBoxProperty` is **compilation-global** and attached to hash-consed
trees, so any given subtree is simplified exactly once per compilation. The
faust-rs port kept the function and the memo but lost the memo's *scope*, which
is the part that made it an optimization.

Two smaller instances of the same pattern exist and should be looked at in the
same pass, though neither is on the hot path in this corpus:

- `crates/eval/src/lib.rs:1211` — fresh cache per `route` spec normalization.
- `crates/eval/src/simplify.rs:38,162,305` — a fresh `ArityCache` per
  `propagate_box_and_simplify` call, which is itself called from inside the
  simplification recursion. This is a *second* layer of the same loss: each
  candidate node re-runs a full typed propagation with an empty arity cache.

### 2.3 Confirming experiment

Making the memo persistent (a five-line `thread_local`, purely to measure) and
re-running:

| | before | after | |
|---|---|---|---|
| `reverb_designer` | 7370 ms | **849 ms** | 8.7×; C++ is 871 ms |
| `spectral_level` | 987 ms | 849 ms | |
| `vcf_wah_pedals` | 837 ms | 750 ms | |
| `phaser_flanger` | 1524 ms | 1540 ms | unchanged — cost is elsewhere |
| corpus total | 18.1 s | **11.2 s** | ratio 3.81× → **2.23×** |
| median delta | +122 % | +103 % | |

The cpp impulse lane stayed 94/94 under the experiment.

**The experiment is not the fix.** A `thread_local` outlives a single
compilation, and `TreeId`s from two different arenas are different trees with
the same integer. The CLI compiles once per process and would be fine; the FFI
factories, the test suite and any embedder compile many times per process and
would read another compilation's memo. The scope has to be the arena, which is
also what makes the invalidation automatic.

---

## 3. Design

### 3.1 Where the memo belongs

`tlib::PropertyStore<T>` already exists (`crates/tlib/src/property.rs`) and is
documented as the port of `compiler/tlib/property.hh` +
`CTree::setProperty/getProperty` — exactly the C++ mechanism above. It is
currently used only by the parser, for source locations.

Two options:

| Option | Shape | Assessment |
|---|---|---|
| **A. Memo on the arena** | `TreeArena` gains a `PropertyStore`-backed simplification memo; `box_simplification` drops its `cache` parameter and uses `&mut TreeArena`, which it already takes | Closest to upstream, where the memo *is* a node property. No signature changes at any call site. Lifetime is the arena's, so a new compilation cannot see an old memo. Puts one eval-specific slot in `tlib` — which is what C++ does, and `PropertyStore` exists for it |
| **B. Explicit memo threaded through eval** | An `EvalMemo` created once in `eval_entrypoint_full` and passed down | Keeps `tlib` free of eval concerns, but touches every frame between the entry point and `apply.rs`, and the evaluator's recursion is deep and varied. Larger diff, same effect |

**Recommendation: A.** It is the upstream shape, the mechanism is already
ported, and it removes a parameter rather than adding one. B's only advantage
is layering purity, and C++ resolves that question the same way A does.

### 3.2 Why the memo is sound

`numeric_box_simplification(arena, cache, box_id)` reads no context beyond
`box_id`: no environment, no closure, no flags. Trees are hash-consed, so equal
subtrees are the same `TreeId`, and simplification is deterministic. Therefore
`box_id ↦ simplified` is a function, and caching it for the arena's lifetime
changes nothing observable.

Three obligations the implementation must discharge, each of which is a way the
above could stop being true:

1. **Arena identity.** The memo must live in the arena, not beside it. A memo
   keyed by `TreeId` and stored anywhere with a longer life is wrong, and wrong
   *silently* — it returns a valid-looking tree from another compilation.
2. **Growth only.** The arena interns and never rewrites nodes, so a memoized
   entry cannot go stale. If a mutation path is ever added, the memo must be
   invalidated with it. Worth an assertion rather than a comment.
3. **No context creeps into the simplifier.** If `numeric_box_simplification`
   ever gains a parameter beyond `box_id`, the key must gain it too. The
   existing plan of 2026-07-04 §3 documents exactly this failure mode for
   `propagate_in_slot_env`, where a `suppress_fad` side channel makes the
   naive key unsound — and its answer, bypassing the memo rather than widening
   the key, is the right precedent.

Note a pre-existing divergence found while reading: C++ `boxSimplification`
transfers the def-name property from the original box to the simplified one;
faust-rs does not. That is not caused by this work and does not block it, but a
memo makes the transfer happen once instead of per call, so it should be
settled in the same pass rather than left implicit.

---

## 4. Phases

### P0 — measurement harness — done 2026-08-06

`cargo run -p xtask -- compile-profile`
(`crates/xtask/src/compile_profile.rs`). Compiles every corpus DSP in-process
and collects per-stage durations from the compiler's own timing sink rather
than by parsing a subprocess's stderr, so stage names cannot drift out of sync
with the compiler. `--write` records a profile, `--baseline` compares against
one and fails on drift.

It compares **shares, not seconds**: shares are dimensionless, so a baseline
survives a machine change and a faster runner does not read as drift — the
failure mode `compile_budget`'s own header documents for absolute ceilings.
Absolute cost stays `compile-budget-check`'s job; duplicating it here would
produce two gates failing together for unrelated reasons.

Baseline recorded at `tests/compile-profile/corpus-baseline.json`: 133 DSPs,
21.2 s, evaluation 71.5 %, parser 14.0 %, signal-fir 12.2 %. The shares differ
slightly from §1.1 because the harness profiles the whole 133-DSP corpus while
§1.1 was measured by hand over the 94 DSPs `compile-bench` gates; the shape is
the same and the harness is now the reproducible number.

**The harness immediately paid for itself.** Re-applying the §2.3 experiment as
its rejecting mutation did not produce a share shift — it produced a **stack
overflow**. Compiling 133 programs in one process is exactly the situation a
`thread_local` memo cannot survive: `TreeId`s from a previous arena resolve to
unrelated trees, and the evaluator recurses until the stack runs out. This is
obligation V3 of §5, demonstrated rather than argued, on the harness's first
use — and the CLI cannot show it, because it compiles once per process. The
drift comparison was then verified separately against a perturbed baseline
(exit 1) and by four unit tests over the comparison itself.

### P1 — arena-scoped simplification memo — done 2026-08-06

Option A as designed. `TreeArena` owns a `PropertyStore<TreeId>` and exposes
`property_key` / `node_property` / `set_node_property`; `box_simplification`
lost its `cache` parameter and both call sites lost their per-call allocation.

Measured, against the prediction of ~11 s / ~0.85 s:

| | before | after | |
|---|---|---|---|
| corpus (`compile-bench`, 94 DSPs) | 18.11 s | **10.68 s** | ratio 3.81× → **2.30×** |
| median per-DSP delta | +122 % | +101 % | |
| `reverb_designer` | 7.226 s | **0.754 s** | C++ is 0.842 s — **faster than the reference** |
| `compile-profile` (133 DSPs) | 21.2 s | 13.7 s | evaluation share 71.5 % → 56.5 % |

Correctness: cpp/c/interp/wasm impulse lanes 94/94; `golden-check` byte-identical
(V2); `emission-determinism` 399 stable; `vector-coverage-check` 1568 pairs;
`cli-transcript-check` 148 identical; workspace tests and clippy clean.

`compile-budget-check` baselines were retightened — every entry decreased, none
increased. `reverb_designer` scalar went 270.6 → 27.3 units (−90 %) and the
front-end basket fell 4–22 % across the board. Leaving the old ceilings in place
would have let the regression back in unnoticed, which is the failure the budget
exists to prevent.

**Two things the mutation testing caught, neither of which the first attempt
had right.**

The V3 test initially failed to reject its mutation twice over. First, toy
programs do not discriminate: four hand-written pairs all passed under a
`thread_local` memo, because small arenas' low `TreeId`s are canonical nodes
that simplify to themselves in any arena, so the leak returns the right answer
by accident. Real corpus DSPs were needed — `freeverb` then `spectral_tilt`.
Second, and worse, the test gave each compilation its own worker thread for
stack reasons; `thread_local` is per-thread, so thread-per-compile *isolated
exactly the leak the test existed to catch*. Sharing one worker across the
scenario is what makes it evidence. Under the mutation it now fails with
`boxPar expects child node 15847 to exist in the bound TreeArena`.

A test that passes under its own mutation is worse than no test, because it is
recorded as coverage.

### P2 — profiled 2026-08-06; hypothesis refuted, nothing implemented

P2 proposed memoizing `propagate_box_and_simplify`'s per-call `ArityCache`.
**That hypothesis is wrong**, and so was the profiling method that suggested it.
Nothing was implemented; the phase's output is a measurement and a correction.

**Two candidate fixes were measured and rejected.**

`propagate_box_and_simplify` is cold after P1 — 242 of 7888 samples, against
`box_simplification`'s 322. There is no second layer to fix there.

The `liftn` closed-subterm fast path proposed by
`porting/cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md` §P2 was
implemented as specified and measured: corpus 14.19 s mean over three runs
without it, 14.49 s with. It is **not a win**. Instrumenting the call site
explains why — `liftn` is called fewer than a thousand times per compilation,
and almost all of those return at its existing `(root, threshold)` memo probe
before reaching any guard. The plan's "two-line change" attacks a loop that is
not hot. That finding belongs to that plan and is recorded here because this is
where it was measured.

**The method that produced the wrong hypothesis.** §2.1 attributed samples by
taking, for each symbol, the largest count on any line of `sample`'s call-graph
output. That number is a *stack node's cumulative total*, not self time, so a
symbol sitting on a hot deep stack reads as hot without doing any work. It put
`liftn` and `tlib::clone_rec` at ~41 % each; their true self times are
`clone_rec` **35** samples and `liftn` below the 5-sample reporting floor.
`sample` already prints a sound self-time ranking under *"Sort by top of stack,
same collapsed"*, and that section is what any future profiling here should
read.

**What the sound profile says.** Self time on `phaser_flanger`, post-P1:

| frame | samples | share |
|---|---|---|
| `lrlex::lexer` | 1836 | 23 % |
| `regex_automata::hybrid::search::find_fwd` | 1793 | 23 % |
| `regex_automata`, two more frames | 1168 | 15 % |
| allocator (`tiny_malloc`/`tiny_free`/…) | ~1000 | 13 % |
| `sigtype::rules::TypeAnnotator::infer` | 99 | 1 % |
| `tlib::arena::clone_rec` | 35 | 0.4 % |

Lexing and its regex engine are ~61 % of what remains. Confirmed independently
without a profiler:

| program | parser | evaluation | total |
|---|---|---|---|
| `import("stdfaust.lib"); process = os.osc(440) : fi.lowpass(1,1000);` | 0.018 s | 0.226 s | **249.5 ms** |
| same shape, no import | 0.003 s | 0.000025 s | **3.8 ms** |

The standard library is re-lexed on every compilation, lazily, during the
*evaluation* stage — which is why evaluation carries the cost and why the
`parser` stage looks small. This also revises §7: that document bounded the
2026-07-04 plan's value by the `propagation` stage's 2.2 %, but stage names do
not locate functions, and the same reasoning error is what produced this
phase's hypothesis.

### P2′ — library lexing — measured 2026-08-06

Measured with `cargo run --release -p parser --example lexbench`, which reports
all three numbers below and is kept as the evidence.

**The lexer is 373× slower than a straightforward scanner.**

| | throughput | per token |
|---|---|---|
| `lrlex` over the installed `.lib` corpus (2.2 MB, 227 526 tokens) | 2.4 MB/s | 4.08 µs |
| minimal hand-written reference scanner, same bytes (228 384 tokens) | 911 MB/s | 0.011 µs |

The reference recognizes whitespace, both comment forms, identifiers, numbers,
strings and punctuation — not a Faust lexer, but it produces a near-identical
token count on the same input, so the comparison is not measuring different
amounts of work. `lrlex` matches each of `faustlexer.l`'s 128 rules with its own
`regex_automata` automaton and takes the longest, which is where the factor
comes from.

**How much is lexed.** A two-line DSP that imports `stdfaust.lib` lexes
**453 836 bytes** across ten files — `filters.lib` 162 KB, `basics.lib` 116 KB,
`oscillators.lib` 76 KB, `maths.lib` 44 KB. At 2.4 MB/s that is ~180 ms of a
249 ms compile.

**Done: the lexer definition is now built once.** `lexerdef()` compiles the 128
rules into automata and measured 2.3 ms — and it was called once per *file*, so
that two-line DSP rebuilt a constant ten times, 23 ms of its 249 ms. It is now a
`OnceLock`. Sound because the definition is immutable: `lexerdef.lexer(input)`
borrows it and all mutable state — position, the `comment`/`doc`/`lst` start
conditions — lives in the returned lexer. Corpus 14.19 s → 12.9 s, and
`compile-bench` 2.30× → **2.13×**.

**Why `lrlex` is slow, and it is not implementation quality.** `lrlex` keeps the
128 rules as 128 *separate* anchored regexes and, at every token start, runs
each one that the current start condition allows, keeping the longest match
([`lexer.rs:419`](file:///Users/letz/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/lrlex-0.13.10/src/lib/lexer.rs)).
That is O(tokens × rules) automaton startups. `flex`, which C++ Faust uses,
compiles its 160 rules into **one** table-driven DFA of 611 states
(`yy_accept`/`yy_base`/`yy_def`/`yy_nxt`/`yy_chk` in
`compiler/parser/faustlexer.cpp`) and does one table lookup per input byte:
O(bytes), independent of rule count. The measured gap follows from the
asymptotics, not from tuning.

Same DSP through both compilers, confirming it end to end:

| | parser | evaluation | total |
|---|---|---|---|
| C++ Faust 2.87.1 | 0.48 ms | 10.4 ms | ~20 ms |
| faust-rs | 9.3 ms | 236 ms | ~230 ms |

Both defer library reading to *evaluation* (C++ without the import: 0.048 ms),
so this compares the same work.

**The combined DFA is available without leaving the current dependencies.**
`regex-automata` — already a dependency, pulled in by `lrlex` itself — supports
multi-pattern automata through `new_many`, returning which pattern matched.
Building one lazy (`hybrid`) DFA over the same 128 rule patterns and scanning
the same corpus:

| strategy | build | throughput | vs `lrlex` |
|---|---|---|---|
| `lrlex`, 128 separate regexes | 2.3 ms | 2.4 MB/s | — |
| one lazy multi-pattern DFA | **0.6 ms** | **266 MB/s** | **112×** |
| one fully-determinized `dense::DFA` | 79 s | 240 MB/s | 98× |
| hand-written reference scanner | — | 781–920 MB/s | ~350× |

The lazy DFA is the shape a replacement should take: it builds in under a
millisecond and determinizes on demand, where the dense DFA would have to be
serialized at build time — which is, in effect, what `flex` does.

**Not done: replacing the lexer.** The headroom is real and it is the largest
remaining item by a wide margin — removing essentially all lexing time would
take the corpus from ~12.9 s toward ~6 s. But throughput probes are not lexers.
A replacement has to reproduce `faustlexer.l` exactly: longest-match with
earliest-rule tie-breaking, the four exclusive start conditions, and the `mdoc`
sublanguage. The benchmark's combined-DFA loop implements none of that, and its
match count (114 730) differs from `lrlex`'s token count (227 526) precisely
because it does not. This is a port with its own plan and its own differential
gate — lex every corpus and library file with both and compare token streams —
not an afternoon.

**Also found, not done: files are parsed more than once per compilation.** The
trace above shows `platform.lib` parsed three times and `maths.lib` twice —
50.7 KB of the 453.8 KB is redundant, 11 %. A per-compilation parsed-file memo
is cheaper than a new lexer and independent of it. It needs care around source
origins and metadata, which are threaded through `parse` and are not obviously
a function of the file alone.

**Still not to be attempted casually: reusing parsed libraries *across*
compilations.** Anything keyed by `TreeId` cannot cross an arena, for the reason
P1 documents; such a cache would have to hold source-level ASTs and re-intern
per compilation, and would need V3-style evidence that it does.

### P3 — parser

2.84 s, 26 % of the post-P1 total. Dominated by machine-expanded corpus files:
`modulations.dsp` is 922 KB of which **one line is 917 KB**, parsed in 0.95 s.
This is a property of the test corpus rather than of real DSP sources, so P3 is
lower value than its share suggests, and should be judged on a corpus of
hand-written sources before any work is done.

### P4 — `fir-prepare-normalize`

1.89 s, 17 % post-P1, and the only significant cost inside the signal pipeline.
Unanalyzed.

---

## 5. Validation

Following `porting/` methodology: the producer never validates itself, and each
check must have a mutation that turns it red.

| # | Obligation | Independent check | Rejecting mutation |
|---|---|---|---|
| V1 | Memoization changes no output | Full impulse corpus, every backend, both `--table-init` modes, byte-identical `.ir` against the C++ oracle | — (this is the numeric gate) |
| V2 | Emitted code is byte-identical, not merely numerically equal | `xtask emission-determinism` extended to compare pre-/post-memo emission for every corpus DSP | Return `box_id` unchanged from a memo hit for one node kind → emission differs |
| V3 | The memo is arena-scoped | A test compiling two different programs in one process and asserting the second is unaffected by the first. `xtask compile-profile` is already such a test in practice: it compiles 133 programs in one process | Hoist the memo to a `thread_local` (the §2.3 experiment) → **confirmed 2026-08-06**: `compile-profile` dies with a stack overflow |
| V4 | The memo actually hits | Instrumented hit/miss counters on a known input, asserted against a recorded ratio | Disable insertion → hit rate collapses to 0, assertion fails |
| V5 | Compile time actually improves | P0 harness, asserting the corpus total stays under a recorded ceiling | — (regression guard) |

V3 is the one that matters most, because it is the failure the convenient
implementation produces, and it is invisible to V1 and V2: a `thread_local`
memo passes every single-compilation test in the suite. The CLI compiles once
per process, so the impulse corpus — 94 processes, 94 compilations — cannot see
it either. Only a test that compiles twice in one process can.

---

## 6. Risks

- **Silent cross-compilation reuse.** Discussed above; V3 exists for it.
- **Memory.** The memo is one `Option<TreeId>` slot per node per key.
  `PropertyStore` is already `Vec`-indexed by `TreeId`, so this is bounded by
  arena size and should be measured, not assumed, on `modulations` (the largest
  corpus input).
- **`PropertyStore` capacity behaviour under a hot path.** It was written for
  the parser's source locations, which are written once and read rarely. This
  memo is the opposite. If its layout turns out to be the wrong shape for a hot
  read path, that is a `tlib` change, not a reason to move the memo elsewhere.
- **Scope creep into P2/P3.** P1 is a self-contained change with a measured
  expected effect. P2 and P3 are separate, and P3 may well be worth nothing.

---

## 7. Relation to the 2026-07-04 plan

`porting/cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md` ports the
upstream memoization commits and identifies the main faust-rs gap as "no result
memo in `propagate_in_slot_env`". Its §3 treatment of unsound keys is directly
reusable and was the precedent for P1's soundness obligations.

**Correction (2026-08-06, after P2's profiling).** This section originally
bounded that plan's value by observing that the `propagation` *stage* is 2.2 %
of compile time. That reasoning is invalid: a stage name does not locate a
function, and `propagate_in_slot_env` is called from evaluation, not only from
the stage that shares its name. The bound was wrong.

What replaces it is a direct measurement rather than a better inference. That
plan's §P2 `liftn` fast path was implemented and measured under P2 here: no
win, because `liftn` is called under a thousand times per compilation and
almost always returns at its existing memo.

**Second correction (2026-08-06, later the same day).** The sentence that stood
here — "the self-time profile does not show `propagate_in_slot_env` among the
leaders, which is evidence against it" — was also wrong, and for a new reason:
that profile was of a *corpus* DSP, and the corpus contains nothing shaped like
the programs where propagation dominates. On
`virtualAnalogForBrowser.dsp` (331 lines, 108 widgets) propagation is **82 %**
of compile time and runs **39× slower than the reference**, against 2.2 % on
the corpus. That plan's value was never measured on an input that could show
it. See `porting/propagation-cost-analysis-2026-08-06-en.md`.

---

## 8. References

- `crates/eval/src/apply.rs:168` — the per-dispatch cache allocation.
- `crates/eval/src/simplify.rs:347` — `box_simplification`, memoized on a
  caller-supplied cache.
- `crates/tlib/src/property.rs` — `PropertyStore`, the ported C++ mechanism.
- `/Users/letz/faust/compiler/evaluate/eval.cpp:1603` — `boxSimplification` and
  `gSimplifiedBoxProperty`.
- `tests/impulse-tests/Make.bench` — `compile-bench`, and
  `build/bench/compile-summary.csv` for the per-DSP table.
