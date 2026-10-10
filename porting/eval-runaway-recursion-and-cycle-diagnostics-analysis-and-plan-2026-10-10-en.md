# Evaluator runaway recursion and evaluation cycles: analysis and correction plan

Date: 2026-10-10

Status: **analysis and plan**. Nothing is implemented on `main-dev`. A
prototype of work package WP1 exists, uncommitted, in the working tree of
`main-dev` at the time of writing (section 7.1); it was used to check the
design on the examples below.

Issues:
[grame-cncm/faust-rs#21](https://github.com/grame-cncm/faust-rs/issues/21)
(a non-constant filter order reports a depth-budget error) and
[grame-cncm/faust-rs#22](https://github.com/grame-cncm/faust-rs/issues/22)
(a recursion-cycle diagnostic carries an "unsupported or malformed
intermediate form" cause). Related:
[#16](https://github.com/grame-cncm/faust-rs/issues/16) (deep acyclic
nesting), fixed by `2d477161` on 2026-09-08 but still open, and work package
W5 of the
[2026-10-08 correction plan](foreign-functions-cli-depth-and-cost-correction-plan-2026-10-08-en.md)
(explain the active depth guard).

Compared versions:

| Compiler | Version | Source | Binary used |
|---|---|---|---|
| Faust C++ reference | 2.84.3 | `master-dev-ocpp-od-fir-2-FIR19` at `8eebea429` (2026-02-08), the pinned reference of `AGENTS.md` | `build/bin/faust` of that checkout |
| Faust C++ current | 2.90.6 | `master-dev`; sources read at `origin/master-dev` `61805e389` (2026-10-06) | `/usr/local/bin/faust`, `Source commit: 02d036331` |
| faust-rs | 0.9.0 | `main-dev` at `20669a61`; the evaluator is unchanged since `5856b1eb` | debug and release builds of `5856b1eb`, and of the prototype |

The two C++ versions share the two loop detectors, `isBoxNumeric` and
`simplifyPattern`. Two differences matter here: the compile thread's stack
went from 16 MB to 256 MB (`8d0cc8418`, 2026-09-02), and the propagation memo
became a table cleared at every outermost `boxPropagateSig` call
(`536ff8ca7`, 2026-07-03; section 3.3).

All timings are single runs on an Apple Silicon laptop on battery, often with
other compilations in parallel. Ratios between runs (a factor of 4 when a size
doubles) are robust; absolute times are indicative only.

## 1. Summary

1. **#21 is a recursion that cannot terminate, and both compilers only say
   "stack overflow".** `fi.bandpass(Nh, fl, fu)` recurses through
   `bpbsr(s, O, Nh, fl, fu)`, whose base cases are the numeric patterns `0` and
   `1` for `O`, and whose general rule calls `bpbsr(s, O-2, ...)`. With `O` a
   UI control, no numeric rule can ever match, and the general rule recurses
   until a depth guard stops it. C++ 2.84.3 reports `ERROR : stack overflow in
   eval` for every minimal version of this program (section 2) and runs until
   its timeout on `fi.bandpass` itself. faust-rs reports `FRS-EVAL-0099` with
   help about a missing base case. The evaluator does see the cause: at each
   level it compares a non-numeric argument with numeric patterns. It keeps no
   trace of it.
2. **faust-rs folds a growing argument at quadratic cost.** At each level the
   evaluator tries to fold the argument to a number (C++
   `simplifyPattern`). C++ 2.84.3 memoizes that work on the trees, so folding
   `x - 1` reuses the folding of `x`, and the recursion is linear. faust-rs
   re-propagates the whole argument each time. With the release budget (32 768
   structural frames), the simplest recursions of #21 run into the 120 s CLI
   timeout before the budget, with no diagnostic at all. The same cost hits
   **valid programs**: `g(n-1, x-1)` with `x` a slider tested against `0` takes
   35 s at `n = 4000`, against 0.1 s in C++ 2.84.3. C++ 2.90.6 has regressed to
   the same shape (5.9 s), because of `536ff8ca7`.
3. **#22: a true evaluation cycle falls into the generic diagnostic.**
   `EvalError::LoopDetected` has no dedicated diagnostic arm. It gets the
   catch-all note "evaluator reached an unsupported or malformed intermediate
   form", and its message names an internal node (`recursive evaluation loop
   on node 55`). When it is raised, the evaluator's call stack holds exactly
   the definitions of the cycle (`process → effect → cut → process`), which
   C++ does not report either: it says "endless evaluation cycle of 18 steps".
4. The same catch-all note also reaches `NotAConstantExpression` and
   `Cancelled`, which are about the program or the run, not about a malformed
   internal form.
5. Two side findings. Labels of errors raised inside library code are picked
   from hash-consed identifiers that occur in other files (#21 underlined
   `ma = library("maths.lib")` in `stdfaust.lib`). With `--error-format
   json`, a compilation timeout prints nothing on stdout, only a text line on
   stderr.

## 2. The examples

Each program is self-contained (no library) except the issue's own. They are
small on purpose: each isolates one behavior. Groups: **a**, the problem of
#21; **b**, valid programs that must keep compiling; **c**, failures that must
keep the generic diagnostic; **p**, cost measurements.

```faust
// a01: a filter-like chain whose length is a UI control
chain(0) = _;
chain(n) = chain(n-1) : *(0.5);
process = chain(hslider("order", 3, 0, 8, 1));

// a02: the shape of fi.bandpass: base cases 0 and 1, step 2
bp(0) = _;
bp(1) = *(0.5);
bp(o) = bp(o-2) : *(0.25);
process = bp(hslider("order", 4, 1, 8, 1));

// a03: the control is smoothed, as in the issue (si.smoo)
smooth = *(0.001) : + ~ *(0.999);
process = chain(hslider("order", 3, 0, 8, 1) : smooth);   // chain as in a01

// a04: int() of a control is still a signal
process = chain(int(hslider("order", 3, 0, 8, 1)));        // chain as in a01

// a05: the tested argument is the second one
g(x, 0) = x;
g(x, n) = g(x*0.5, n-1);
process = g(_, nentry("n", 3, 0, 8, 1));

// a06: mutual recursion
even(0) = 1;
even(n) = odd(n-1);
odd(0) = 0;
odd(n) = even(n-1);
process = even(button("b"));

// a07: the order comes from an audio input, through a lambda applied with `:`
process = _ <: \(n).(chain(n));                             // chain as in a01

// issue #21 (needs the standard libraries)
import("stdfaust.lib");
fc = hslider("Center Frequency [unit:Hz]", 1000, 20, 20000, 1) : si.smoo;
q = hslider("Q", 1.0, 0.5, 20, 0.01) : si.smoo;
process = fi.bandpass(q, fc, fc), fi.bandpass(q, fc, fc);
```

```faust
// b01, b02: a constant, a constant expression
process = chain(3);
N = 2 + 1;
process = chain(N);

// b03: a signal at a position that no rule compares with a number
process = g(hslider("x", 0.5, 0, 1, 0.01), 3);             // g as in a05

// b04: a numeric pattern applied to a signal, without recursion
h(0) = 1;
h(x) = x*2;
process = h(hslider("x", 0.5, 0, 1, 0.01));

// b05: finite, descends on n; x is a signal tested against 0 at each level
g(0, x) = x;
g(n, 0) = 0;
g(n, x) = g(n-1, x);
process = g(3, hslider("x", 0.5, 0, 1, 0.01));
```

```faust
// c01: a real missing base case, on constants
f(0) = _;
f(n) = f(n+1) : *(0.5);
process = f(3);

// c02: the recursion skips its base case
f(n) = f(n-2) : *(0.5);   // with f(0) = _;
process = f(3);

// c03: finite, deeper than the debug budget (1024), within the release one
f(n) = f(n-1) : *(0.999); // with f(0) = _;
process = f(5000);

// c04: finite and deep, descends on n; x tested against 0 at each level
g(0, x) = x;
g(n, 0) = 0;
g(n, x) = g(n-1, x) : *(0.999);
process = g(5000, hslider("x", 0.5, 0, 1, 0.01));

// c05: one dispatch on a signal, then a deep recursion on constants
h(0) = _;
h(x) = f(5000);           // f as in c03
process = h(hslider("x", 0.5, 0, 1, 0.01));

// c06: a recursion stopped by a run-time test, with no numeric pattern
f(n) = select2(n > 0, _, f(n-1) : *(0.5));
process = f(hslider("n", 3, 0, 8, 1));
```

```faust
// p1: constant argument, 5000 levels
f(0) = _;
f(n) = f(n-1) : *(0.999);
process = f(5000);

// p2: an argument that grows, never compared with a number, 5000 levels
g(x, 0) = x;
g(x, n) = g(x*0.5, n-1);
process = g(hslider("x", 0.5, 0, 1, 0.01), 5000);

// p6: VALID; an argument that grows and is compared with 0 at each level
g(0, x) = x;
g(n, 0) = 0;
g(n, x) = g(n-1, x-1);
process = g(N, hslider("x", 0.5, 0, 1, 0.01));   // N = 1000, 2000, 4000
```

And for #22:

```faust
// self
x = x;
process = x;

// cycle3
a = b + 1;
b = c * 2;
c = a;
process = a;

// issue #22
import("stdfaust.lib");
cut = hslider("Cut", 0, 0, 1, 0.01) : si.smoo : ba.bypass2(2, process);
effect = par(i, 2, _ * (1 - cut)) : par(i, 2, _ * cut);
process = effect, cut;
```

## 3. What Faust C++ does

### 3.1 Two detectors

`compiler/evaluate/eval.cpp:297`, `eval()`: on every evaluation that misses
the memo, the evaluator calls two detectors before `realeval`.

- `loopDetector::detect(cons(exp, localValEnv))`
  (`compiler/evaluate/loopDetector.cpp:30`), constructed as
  `gLoopDetector(1024, 400)` (`compiler/global.cpp:139`). A ring buffer of the
  last 1024 `(expression, environment)` pairs; every 400 steps it looks for the
  current pair among the previous ones. Since a pair is memoized once its
  evaluation ends, a repeat means its evaluation is still in progress: a
  cycle. Message: `ERROR : after <n> evaluation steps, the compiler has
  detected an endless evaluation cycle of <k> steps`. Nothing names the
  definitions.
- `stackOverflowDetector::detect()` (`loopDetector.cpp:58`): compares the
  current stack address with the first one and throws `ERROR : stack overflow
  in eval` when the distance comes within 256 KB of `MAX_STACK_SIZE`. That is
  16 MB at `8eebea429` (`loopDetector.hh:51`), and 256 MB on `master-dev`
  (`loopDetector.hh:57`, `8d0cc8418`), which is the size of the compile
  thread's stack.

Nothing in C++ relates a depth overflow to a pattern argument that is not a
number.

### 3.2 Folding a pattern argument, memoized at every level (2.84.3)

When a function defined by numeric patterns is applied, its argument is folded
to a number if it can be. `simplifyPattern` (`eval.cpp:136`) caches the
answer on the argument tree (`NumericProperty`). `isBoxNumeric`
(`eval.cpp:814`) runs `a2sb` (memoized: `gSymbolicBoxProperty`), `getBoxType`
(memoized: `BOXTYPEPROP`, `boxes/boxtype.cpp:65`), `boxPropagateSig`, then
`simplify` (memoized: `sigMap(gGlobal->SIMPLIFIED, ...)`,
`normalize/simplify.cpp:78`). At `8eebea429` propagation is memoized as a
property of its argument tuple: `setPropagateProperty` and
`getPropagateProperty` (`propagate/propagate.cpp:164`, `:176`), used by
`propagate()` (`:918`). The property lives as long as the trees.

So when the argument of level `k` is `x_k = x_{k-1} - 1`, folding it
propagates one new node: everything below was propagated, typed and
simplified at level `k-1`. The recursion is linear.

### 3.3 The same folding, no longer memoized across calls (2.90.6)

`536ff8ca7` (Yann Orlarey, 2026-07-03, "tlib+propagate: lazy per-node
properties, fast-path slot, tuple memoization") replaced the property by a
global table, `gPropagateMemo` (`propagate.cpp:128` at `61805e389`), keyed by
plain data for speed. A `PropagateMemoScope` (`:130`), opened by
`boxPropagateSig` (`:853`), clears the table on entry to and exit from the
outermost scope, so that entries do not survive into another libfaust
compilation. During evaluation, each fold of a pattern argument is an
outermost `boxPropagateSig`. The table is therefore empty at every fold, and
each fold propagates the whole argument again: level `k` costs `O(k)`, and the
recursion costs `O(N²)`. The profile of 2.90.6 on a01 is dominated by
`PropagateMemoKey` hash-table inserts and finds and `realPropagate`.

Confirmed by building the C++ compiler at four points (Release, C++ backend
only; p6 best of 3, a01 one run):

| C++ compiler | p6, N = 1000 / 2000 / 4000 / 8000 | a01 |
|---|---|---|
| `afa643dd6`, parent of `536ff8ca7` | 0.02 / 0.04 / 0.08 / 0.16 s | `stack overflow in eval`, 0.10 s |
| `536ff8ca7` | 0.28 / 1.07 / 4.37 s / SIGBUS | SIGBUS after 4.3 s |
| `61805e389` (`master-dev`) | 0.27 / 1.01 / 4.24 / 17.0 s | killed by the 120 s timeout |
| `61805e389` with the two `gPropagateMemo.clear()` calls commented out | 0.02 / 0.03 / 0.07 / 0.14 s | `stack overflow in eval`, 1.84 s |

Disabling the clearing alone restores linear time. At `536ff8ca7`, with the
16 MB stack of that time, the unmemoized propagation recurses one native frame
per node of the argument. The evaluator's stack check does not watch that
recursion, so the stack overflows (SIGBUS, crash report in `propagate` /
`realPropagate`). The 256 MB stack of `8d0cc8418` turned the crash into a
timeout.

### 3.4 Results

| Program | C++ 2.84.3 | C++ 2.90.6 |
|---|---|---|
| a01 to a07 | `stack overflow in eval`, 0.1 to 0.2 s | a01, a02, a06: killed by the 120 s timeout (others not run) |
| issue #21 (`fi.bandpass`) | killed by the 120 s timeout | killed by the 120 s timeout |
| b01 to b05 | OK, 0.0 s | b01: OK |
| c01, c02 | `stack overflow in eval`, 0.1 s | c01: `stack overflow in eval`, 1.7 s |
| c03, c04, c05 | OK, 0.1 s | c03: OK, 0.1 s |
| c06 | `stack overflow in eval`, 0.1 s | killed by the 120 s timeout |
| p1, p2 | OK, 0.1 s | p1: OK, 0.1 s |
| p6, N = 1000 / 2000 / 4000 (valid) | OK, 0.0 / 0.1 / 0.1 s | OK, 0.3 / 1.5 / 5.9 s |
| self, cycle3, issue #22 | `endless evaluation cycle of 2 / 10 / 18 steps` | not run |

Notes:

- In 2.90.6, a01-type recursions go 16 times deeper before the stack check (256
  MB) at a cost per level that now grows with the level (section 3.3). The 0.1
  s "stack overflow" of 2.84.3 becomes a timeout.
- On `fi.bandpass` neither version reaches its stack check. 2.84.3 grows from
  2.0 GB to 2.8 GB of resident memory between 15 s and 29 s, its time spent
  building trees (`CTree::make`, under `addElement` of property lists,
  `simplifyPattern`, `propagate`, `a2sb`). 2.90.6 is at 7.8 GB after 15 s, in
  `CTree::make`. The per-level work of `bpbsr` (closures of its `with` block,
  `tf2sb` sections whose coefficients depend on `O`) is far heavier than in a02,
  and the property lists of 2.84.3 are the cost `master-dev` later removed
  (`gEvalMemo`, whose comment cites a node with 56 000 entries). `-sn` (simple
  names) changes nothing, so the definition-name strings built by `applyList`
  (`eval.cpp:1352`) are not the cause.

## 4. What faust-rs does

### 4.1 Three depth guards, and which one stops #21

`crates/eval/src/loop_detector.rs`:

- `enter` (`:510`), the identity frames of `call_stack`: a `(tree, environment)`
  or `(symbol, environment)` frame already on the stack is a cycle
  (`LoopDetected`); more than `max_depth` frames (`DEFAULT_EVAL_MAX_DEPTH`,
  `:282`: 1 024 in debug, 32 768 in release) is `RecursionDepthExceeded`.
- `enter_structural` (`:535`), a counter for each argument applied to a pattern
  matcher (`apply_pattern_matcher_value`, `crates/eval/src/apply.rs:146`) and
  each structural lowering step, capped at `min(max_depth,
  STRUCTURAL_HARD_MAX_DEPTH)` (`:303`: 4 096 in debug, 32 768 in release).
- `enter_eval`, the syntactic nesting of `eval_value`, 400 000 entries (the
  fix of #16).

With the identity limit at 900 and the structural cap at 700, every #21
program stops at 700: the **structural** counter fires. The identity stack
holds a single frame throughout, because a pattern-matcher application pushes
no identity frame, and the arguments differ at each level anyway (`n`,
`n-1`, ...). Recursion levels reached at the debug budget of 1 024: a01 512
(2 frames per level), a05 256, a06 2 × 256, `fi.bandpass` 102 (`bpbsr` takes 5
arguments, about 10 frames per level).

### 4.2 Why the diagnostic says nothing about the cause

`RecursionDepthExceeded { max_depth }` carries the budget only. Its
diagnostic (`crates/eval/src/error.rs:972`) says "check recursive definitions
for a missing base case or non-decreasing recursive call". That is true but
not actionable for #21, whose definitions are correct: the fault is the
argument. Yet `apply_pattern_matcher_value` knows, at each level, that the
state it dispatches on has numeric constant transitions and that the argument
did not fold to a number (`pattern_matcher.rs:950` folds it; the numeric
transitions are then skipped and the variable rule taken). Nothing records it.

### 4.3 Folding cost: quadratic, in error paths and in valid programs

`apply_pattern_matcher` folds the argument with `simplify_pattern`
(`crates/eval/src/simplify.rs:78`), which calls `propagate_box_and_simplify`
(`:46`). Each call:

- validates the whole box from scratch (`try_build_flat_box`,
  `crates/propagate/src/flat.rs:130`, a fresh `visited` set);
- propagates it with a fresh `ArityCache` and a fresh `PropagateMemo`
  (`crates/propagate/src/api.rs:59`);
- simplifies the resulting signal.

There is no counterpart of C++ `NumericProperty` either. The port has kept
the C++ algorithm and lost its memoization, so a fold costs the size of the
argument. Measured in release, 5 000 levels each:

| Program | Time |
|---|---|
| p1, constant argument | 0.05 s |
| p2, growing argument, never folded | 0.8 s |
| a01 at a budget of 1 250 / 2 500 / 5 000 frames | 0.55 / 2.4 / 10.0 s |

Each doubling multiplies the time by 4.2 to 4.4. Debug, the same, before the
prototype and with it: 7.3 / 31.2 / 120.1 s and 7.1 / 30.0 / 119.6 s at 1 024 /
2 048 / 4 096. The prototype changes the message, not the cost. Extrapolated
to the release budget of 32 768, a01 needs about 430 s, beyond the 120 s
timeout: **in release, the simple recursions of #21 never reach their
diagnostic**. The issue's own program does (3.3 s), because each level costs
about 10 frames.

The same cost hits valid programs whenever a growing non-numeric argument is
compared with a number at each level:

| p6 variant, release | N = 1 000 | N = 2 000 | N = 4 000 |
|---|---|---|---|
| `g(n-1, x-1)` | 1.9 s | 7.9 s | 35.2 s |
| `g(n-1, x*0.5)` | 1.7 s | 2.0 s | 1.9 s |
| `g(n-1, x+x)` | 0.04 s | 0.03 s | 0.03 s |

`x*0.5` stays small: the product folds to a single coefficient. `x+x` doubles
a shared DAG, which the per-call memo already handles. `x-1` builds a chain,
and is quadratic. C++ 2.84.3 compiles the `x-1` case in 0.1 s at `N = 4000`.

### 4.4 #22: what the stack holds when a cycle is detected

`eval_ident_value` (`crates/eval/src/lib.rs:1168`) pushes a `SymbolEnv` frame
when it forces a definition (`:1207`), and a `TreeEnv` frame for an
identifier bound to a box (`:1195`). Tracing the stack at the moment
`LoopDetected` is raised:

| Program | Stack when the repeated frame is entered |
|---|---|
| self | `process > x`, entering `x` |
| cycle3 | `process > a > b > c`, entering `a` |
| issue #22 | `process > effect > cut`, entering `process` |

The cycle is the frames from the first occurrence of the re-entered frame up
to the top: `x → x`, `a → b → c → a`, `process → effect → cut → process`. The
error keeps only the body of the re-entered definition (`LoopDetected { node
}`), and its message prints the node's number. `to_diagnostic` has no arm for
`LoopDetected`; it falls to the catch-all (`error.rs:1049`), whose note "cause:
evaluator reached an unsupported or malformed intermediate form" was written
for malformed internal trees. faust-rs detects the cycle at once, by identity,
where C++ samples every 400 steps. It only has to say what it found.

Labels are partial too. On `x = x; process = x;` the primary label is the `x`
of `process = x` ("failing use"), and `process` gets both "enclosing
definition" and "call site". The definition that loops, `x = x`, is not
labelled.

### 4.5 The catch-all note

Variants of `EvalError` without a diagnostic arm, all given the "unsupported
or malformed intermediate form" cause:

| Variant | What it is | Right cause? |
|---|---|---|
| `MalformedDefinitionNode`, `MalformedListNode`, `MalformedCaseNode`, `EmptyArgumentList`, `InternalError` | internal trees | yes |
| `NonIdentifierParameter`, `NonIdentifierIterationVariable` | a lambda parameter or iteration variable that is not an identifier; the parser rejects both in source (`\(1).(_)`, `par(1, 3, _)` are syntax errors, as in C++), so they come from API-built boxes | acceptable |
| `LoopDetected` | an evaluation cycle (#22) | no |
| `NotAConstantExpression` | an expression that must fold to a number and does not (`simplify.rs:124`, `:141`, `:159`) | no |
| `Cancelled` | `--timeout` or a host cancellation | no |

### 4.6 Labels of errors raised in library code

`maybe_add_eval_source_labels` (`crates/compiler/src/diagnostic_enrichment.rs:194`)
labels the definition that owns the failing node when it finds one among the
program's top-level definitions. That works for a01: definition `chain`, call
in `process`. For a function local to a `with` of a library (`bpbsr`), there
is no owner. The fallback, `source_span_from_node_or_descendant` (`:358`),
takes the first located descendant of the node. The rules of a `case` carry
no location of their own, so it picks an identifier, and identifiers are
hash-consed across files. For #21 it underlined `ma = library("maths.lib")` in
`stdfaust.lib` as "call site", and the program's `process` as "definition
site".

### 4.7 A timeout under `--error-format json`

`faust-rs --check --timeout 3 --error-format json a01_slider_order.dsp`
exits 1 with an empty stdout and `ERROR: compilation timeout (3.0s > 3s
limit) after phase 'check'` on stderr. A tool reading the JSON channel, such
as the diagnostic sweep behind #21 and #22, gets no document.

### 4.8 Results

| Program | faust-rs, debug | faust-rs, release | Prototype (WP1), debug |
|---|---|---|---|
| a01 to a07 | `FRS-EVAL-0099` stack overflow (budget 1 024) | killed by the 120 s timeout | `FRS-EVAL-0012`, argument and control named |
| issue #21 | `FRS-EVAL-0099`, 0.2 s | `FRS-EVAL-0099`, 3.3 s | `FRS-EVAL-0012`, 0.2 s |
| b01 to b05 | OK | OK | OK |
| c01, c02, c06 | `FRS-EVAL-0099` | `FRS-EVAL-0099`, 0.07 s | `FRS-EVAL-0099` |
| c03, c04, c05 | `FRS-EVAL-0099` (finite, deeper than the debug budget) | OK, 0.05 to 0.07 s | `FRS-EVAL-0099` |
| self, cycle3, issue #22 | `FRS-EVAL-0099` `recursive evaluation loop on node <n>`, wrong cause | same | unchanged |

## 5. Root causes

- **R1.** The depth guards report a budget, never a cause, and the evaluator
  keeps no trace of the one cause it can establish: a recursion dispatching a
  non-numeric argument on numeric patterns.
- **R2.** Folding a pattern argument is not memoized across calls. C++ 2.84.3
  memoizes it at four levels. The port lost all of them, and C++ `master-dev`
  lost the propagation one in `536ff8ca7`.
- **R3.** `LoopDetected` has no diagnostic arm, and its error does not carry
  the cycle the evaluator has on its stack.
- **R4.** The catch-all cause note applies to every variant without an arm,
  including three that are not internal.
- **R5.** Label fallback trusts the first located descendant, which for
  hash-consed identifiers may be in another file, and names the roles of the
  fallback labels backwards.
- **R6.** The timeout path bypasses the JSON diagnostics channel.

## 6. What a check "ahead" of evaluation could and could not do

The reporter asks whether a constant-ness check ahead of this evaluation path
is in scope. Faust has no type for "compile-time constant parameter": nothing
in `bandpass(Nh, fl, fu)` says that `Nh` must be a number, other than its
documentation and the patterns of a helper three calls below. A check before
evaluation would need such an annotation, a language or library change outside
this plan. Evaluation itself, though, establishes the fact as soon as a
numeric pattern meets a non-numeric argument. Using it when a budget runs out
(WP1) needs no annotation and changes no accepted program.

## 7. Correction plan

### 7.1 WP1, #21: name the argument of a recursion that cannot reach its base case (P1)

Owner: `eval`, diagnostics.

1. In `apply_pattern_matcher_value`, fold the argument once
   (`simplify_dispatch_argument`) and record, on a stack in `LoopDetector`
   pushed and popped with the existing structural frame, every dispatch on a
   state that has numeric patterns: the rules, the 1-based argument position,
   and either "a number" or the non-numeric argument with the numeric patterns
   it cannot reach. Nothing is recorded for other dispatches.
2. When any depth guard runs out (`enter`, `enter_structural`, `enter_eval`),
   return `CaseArgumentNotConstant` instead of `RecursionDepthExceeded` if one
   `case` is being applied, nested at least 3 times, with a non-numeric
   argument at the same position, **and no application of that `case` from
   the first of those on dispatches any argument on a number**. The second
   condition is what keeps c04 generic: it descends on `n` while `x`, a signal,
   is compared with `0` at every level, and it is finite. The outermost
   application is reported. Since the budget has already run out, the outcome
   of the compilation never changes; only the message does.
3. Message: `recursion never reaches its base case: argument 2 is matched
   against the numbers 0 and 1, but is not a compile-time constant` (wording
   to review, D2). Notes: the cause, the rule, the budget. Guidance, which has
   the arena: the controls or inputs the argument depends on
   (`hslider("Q", 1, 0.5, 20, 0.01)`, "an input signal") rather than the
   evaluated box, whose closures and slots have no source spelling; the
   declared rules. Facts: `case_argument_position`, `case_argument`,
   `case_numeric_patterns`, `pattern_rules`. Help: pass a constant number.
4. Labels: the owner definition and the call in `process` when the function
   is a top-level definition; otherwise the call in `process` only, with a note
   that the function comes from a library or a `with` (until WP5).
5. Code: `FRS-EVAL-0012` (D1).

The prototype implements 1 to 5. On the examples: a01 to a07 and the issue
report `FRS-EVAL-0012` with the right position, the numbers and the control;
b01 to b05 compile; c01 to c06 keep `FRS-EVAL-0099`. A first version without the
second condition of step 2 misreported c04, which is why the condition is
there.

Pass criteria: the a, b and c examples as tests in
`crates/compiler/tests/diagnostic_errors.rs` (message, code, facts, label
roles), a corpus fixture `tests/corpus/err_34_*.dsp` with its golden
fingerprint, unit tests of the attribution rule in `loop_detector.rs`
(threshold, numeric progress, mutual recursion), `docs/diagnostics-codes-reference-en.md`,
a registry entry (Rust-only diagnostic; C++ reports `stack overflow in eval`
or runs until its timeout), the public-API baseline, and all gates. WP1 is
useful in release only with WP2: without it, the simplest recursions time out
first.

### 7.2 WP2: memoize the folding of pattern arguments for the whole evaluation (P1)

Owner: `eval`, `propagate`, `normalize`.

Target: C++ 2.84.3 behavior, a fold costing the size of what is new in the
argument.

1. A fold context owned by the `LoopDetector`, for the lifetime of one
   evaluation pass: the boxes already validated as flat, an `ArityCache`, the
   propagation results of 0-input boxes, and the simplified signals.
   `propagate_box_and_simplify` takes it instead of building fresh ones. Keys
   are hash-consed `TreeId`s, immutable during the pass, so entries stay valid;
   the context is dropped with the detector.
2. A memo of `simplify_pattern` results by `TreeId`, the counterpart of C++
   `NumericProperty`, for arguments folded more than once.
3. The propagation memo already has rules for when its results may be reused
   (`result_memo_is_safe_root`). A persistent memo must keep them, and must
   key on everything a 0-input fold depends on (the slot environment and the
   path, empty for these folds). This is the main design point.

Rejected: stopping a recursion early after a fixed number of non-progressing
levels. It would make the diagnostic fast without WP2, but a program whose
argument becomes a number after more levels (`x*0` folds) would be refused
although it terminates, and valid programs (p6) would stay quadratic.

Pass criteria: a01 at 5 000 levels within a small factor of p1 (linear); p6
at `N = 4000` within a small factor of C++ 2.84.3; a01 reports `FRS-EVAL-0012`
in release within the timeout; no generated output changes
(`golden-check`, impulse suite); `compile-budget-check` no worse, and a case
of the p6 shape added to its basket so that the cost cannot come back (the
gate currently cannot run on a fast machine: its calibration DSP takes 3 ms,
below its 4 ms floor; see `HANDOFF.md`).

### 7.3 WP3, #22: name the cycle (P2)

Owner: `eval`, diagnostics.

1. Keep, next to `call_stack`, the symbol of each frame (for `TreeEnv` frames,
   the identifier `eval_ident_value` is resolving), without changing frame
   identity.
2. On a repeated frame, build the cycle from the first occurrence up, and
   raise `LoopDetected` with the node of the use that closes the cycle and the
   names of the cycle.
3. Message, close to C++'s words, without node numbers: `endless evaluation
   cycle: process → effect → cut → process`. Cause: the definitions refer to
   each other with nothing in between that ends the recursion. Help: a
   definition cannot use itself through other definitions; feed a signal back
   with `~`.
4. Labels: the use that closes the cycle, and each definition of the cycle
   that is a top-level definition.
5. Code: a dedicated one, `FRS-EVAL-0013`, or `FRS-EVAL-0099` with a detail
   code (D1).

Pass criteria: tests for a cycle of 1, 2 and 3 definitions, a cycle through a
`with`, through a function argument (the issue's `ba.bypass2(2, process)`,
written without the libraries), and a recursion through `~` that compiles; a
corpus fixture; registry entry; docs.

### 7.4 WP4: give the remaining user-facing errors their own cause (P3)

Owner: diagnostics.

`NotAConstantExpression`: the expression must fold to a number at compile
time, and the help of `FRS-EVAL-0011`. `Cancelled`: the compilation was
cancelled, by `--timeout` or by the host. The catch-all note then stays with
the internal variants only, and says so ("internal: ..."). Pass criteria: one
test per variant that a program or the API can reach.

### 7.5 WP5: labels for errors raised in library code (P3)

Owner: compiler diagnostics.

In the fallback of `maybe_add_eval_source_labels`, do not take a span from
an identifier leaf, whose location may be any of its occurrences in any file.
Name the fallback roles correctly. When the failing code is in a library,
label the program's own call that leads into it. This generalizes the targeted
fix of WP1 step 4. Pass criteria: a `PatternMatchFailed` and a WP1 error
inside a library function, with labels only on located, relevant spans.

### 7.6 WP6: a JSON document for a timeout (P3)

Owner: CLI. Part of W2 of the 2026-10-08 plan (the deadline machinery):
`--error-format json` must emit a diagnostics-v2 document when the
compilation times out.

### 7.7 Order

WP1 and WP3 are independent, small, and can land in one series, with WP4.
WP2 should follow immediately: it is what makes WP1 visible in release, and it
fixes valid programs. WP5 and WP6 can wait.

## 8. Decision points

- **D1.** Dedicated codes `FRS-EVAL-0012` (WP1) and `FRS-EVAL-0013` (WP3),
  like `FRS-EVAL-0011` for widget parameters, or `FRS-EVAL-0099` with detail
  codes? Dedicated codes let a tool recognize the failure without parsing the
  message; the diagnostic sweep behind #21 and #22 is such a tool.
  Recommendation: dedicated codes.
- **D2.** WP1 wording: "never reaches its base case" states an inference. The
  observed facts are that the budget ran out and that the argument is not a
  number. "Does not reach" or "cannot reach" are alternatives.
- **D3.** WP2 scope: the persistent fold context (7.2, item 1) is the C++
  2.84.3 design; item 2 alone does not fix growing arguments.
- **D4.** Report to the C++ side the folding regression of `536ff8ca7`,
  established by the builds of section 3.3 (p6 at `N = 8000`: 0.16 s → 17 s;
  a01: `stack overflow in eval` in 0.1 s → crash at `536ff8ca7`, timeout on
  `master-dev`), together with a growth criterion for `tests/TESTING.md`.
  `fi.bandpass` with a non-constant order, which builds trees until the
  timeout in both C++ versions, is a separate observation.
- **D5.** Close #16, fixed by `2d477161`.

## 9. Reproduction

The programs are those of section 2. Commands:

```sh
faust-rs --check program.dsp
faust-rs --check --error-format json program.dsp
FAUST_RS_DEFAULT_EVAL_MAX_DEPTH=900 FAUST_RS_STRUCTURAL_HARD_MAX_DEPTH=700 faust-rs --check program.dsp
faust -t 120 program.dsp -o /dev/null
```

`FAUST_RS_*` set the identity and structural budgets separately (section
4.1); `-t` is the C++ timeout. Profiles were taken with macOS `sample`.
