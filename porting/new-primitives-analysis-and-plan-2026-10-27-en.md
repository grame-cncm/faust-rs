# New primitives for faust-rs: analysis and general plan

Date: 2026-10-27. Base: `main-dev` at `5a53a5f0`.

Status: **analysis and plan, nothing implemented.** It gathers five
candidates: three already designed in `porting/` and two new. It checks
each one against the code as it stands, orders them, and lists the
decisions that must be taken before any of them starts. Every behaviour
quoted below as "measured" was run on this date with the release
`faust-rs` and `faustprobe` binaries of the base commit, and with the
pinned C++ reference (2.84.3, `8eebea429`).

Related documents:
[`fad-rad-seed-semantics-analysis-2026-09-21-en.md`](fad-rad-seed-semantics-analysis-2026-09-21-en.md) §6.2,
[`nffunction-multiple-outputs-proposal-2026-10-08-en.md`](nffunction-multiple-outputs-proposal-2026-10-08-en.md),
[`repeat-counted-loop-with-exit-analysis-and-plan-2026-09-18-en.md`](repeat-counted-loop-with-exit-analysis-and-plan-2026-09-18-en.md),
[`interleave-spectral-primitive-2026-07-07-en.md`](interleave-spectral-primitive-2026-07-07-en.md),
[`faust-rs-vs-faust-cpp-differences-en.md`](faust-rs-vs-faust-cpp-differences-en.md),
[`../docs/fad-note-en.md`](../docs/fad-note-en.md),
[`../docs/rad-note-en.md`](../docs/rad-note-en.md).

## 0. Summary

| | Candidate | Kind | State on the base commit | Recommendation | Order |
|---|---|---|---|---|---|
| A | `detach(x)`, stop-gradient | AD primitive, keyword | specified (§6.2 of the seed-semantics analysis), not implemented | do first | 1 |
| C | known math functions of the libraries (`tanh`, `cbrt`, `hypot`, `erf`, …) | compiler knowledge, **no syntax** | recognized by name in fourteen files, three copies of the derivative formulas, no interval | one classifier table, then intervals, then AD rules | 2 |
| B | `withgrad(F, G)`, user-given derivative | AD primitive, keyword | not designed until now | after A and C-3 | 3 |
| D | `nffunction`, N→M external function | external call, keyword | full proposal, not implemented, syntax/ABI not validated | after A–C, own validation first | 4 |
| E | `repeat`, loop with exit | clocked wrapper, keyword | semantics reproducible by the library form; primitive kept for codegen only | not before a measured codegen need; **the name must change** (§6) | 5 |

Not retained: a frame or vector signal type, a native `selectn`, native
nodes for library functions defined by formula (§7).

Two findings of this analysis need action whatever is decided about new
primitives:

- **`fad` silently differentiates an unrecognized `ffunction` to zero, and
  `rad` rejects it** (§4.2, measured). The RAD diagnostic tells the user to
  use `fad` "which has the same coverage", which is true of the rule set
  and misleading about the result.
- **`repeat` is not a free name.** `signals.lib` has defined `si.repeat`
  since faustlibraries `9aa3e8f8` (2023-10-11), in the libraries of the pinned
  reference and of upstream 2.90. Reserving `repeat` as a keyword would make
  `stdfaust.lib` fail to parse. D1 of the `repeat` plan states the opposite
  and is wrong (§6).

## 1. Constraints common to every candidate

### 1.1 A keyword is a reserved word

Every keyword added to `faustlexer.l` is reserved in every program. That
follows from the decision of 2026-10-16: no contextual keywords; the parse
error proposes a rename. `fad`, `rad`, `cinputs`, `cinput`, `coutputs` and
`coutput` already cost that: a C++ program that names one of them no longer
parses with faust-rs (DIFF-SRC-001, -002, -004). Two rules follow.

1. **Prefer compiler knowledge to syntax.** A candidate that can be
   delivered without a keyword (C) takes no name away from anyone.
2. **Check a name before choosing it.** "Unused" means: no definition or
   use as an identifier in the standard libraries (those of the pinned C++
   checkout and of upstream 2.90), in
   `libraries/` and `tests/` of this repository, and in the sibling
   `faust-diff-*` projects. Checked on 2026-10-27:

   | name | standard libraries | faust-rs | siblings |
   |---|---|---|---|
   | `detach` | free | free | free |
   | `withgrad` | free | free | free |
   | `nffunction` | free | free | free |
   | `repeat` | **`si.repeat`, `signals.lib:187`** | free | free |

### 1.2 Gates and registries

All candidates touch the compilation pipeline. Each landing runs:

- the AGENTS gates: fmt, clippy `-D warnings`, the workspace tests,
  `golden-check`, and the five structure checks;
- the compile-cost discipline. `compile-budget-check` cannot run on the
  development Mac (the calibration case is under the 4 ms floor), so the
  pre/post A/B of both baskets is done by hand there and the gate itself on
  CI;
- the registries:
  - a new `DIFF-SRC-*` entry for each new source form;
  - a `DIFF-BEH-*` entry for C-2, which makes programs compile that C++
    rejects;
  - a `public-api-baseline.txt` diff, reviewed, for any new `pub` item;
  - a `MEMOIZATION.md` entry if a memo is added (none is planned).

### 1.3 Method and assurance level

House method (plan → check written first → producer → a hand-applied
mutation the check must reject → qualification). Standard assurance tier:
none of these phases emits a structural artifact consumed downstream, so the
certificate/checker methodology of the Lean plan does not apply. The
independent checks are:

- finite differences, for every derivative rule;
- dense sampling, for every interval rule;
- a codegen-identity differential (base against new, every `.dsp` of
  `tests/` and `examples/`), for every change meant to leave outputs
  unchanged.

## 2. A — `detach(x)`, a stop-gradient

### 2.1 The need

Two notes call for it, independently.

- **The seed-semantics analysis**, §6.2: a stop-gradient is the precondition
  for any later change to seed semantics, and every AD framework has one
  (`detach`, `stop_gradient`, `ignore_derivatives`).
- **faust-diff-demo**, in `notes/faust-rs-descent-in-recursion-en.md`, from
  its leader/scout pair. Without a cut, the leader's `fad` differentiated the
  whole scout (its solver and its own `fad` included), and the compilation
  ran past five minutes and five gigabytes. The workaround in
  `dsp/ts808_xray_pair.dsp:78` is
  `opaque(v) = float(int(v * 1048576.0)) / 1048576.0`. It works because
  `fad` crosses an integer cast with a zero tangent, and the compilation then
  takes one second. But it quantizes the value to 2⁻²⁰.

`detach` has to give the value unchanged, and the compile time the cast
gives.

### 2.2 Semantics

`detach : 1 → 1`.

| context | effect |
|---|---|
| outside any `fad`/`rad` | identity; erased before normalization |
| `fad` | primal: the original `x` signal, **not transformed**; tangent `0` on every lane |
| `rad`, symbolic sweep | no adjoint propagated below the node |
| `rad`, `BlockReverseAD` | a stop of the block sweep (`collect_bra_postorder`'s `stops`, `crates/transform/src/signal_fir/block_reverse_ad.rs`); identity in the primal replay |
| nested `fad`/`rad` | a stop at every order: the node stays in the primal an inner differentiation returns, so an outer one meets it too |

The "not transformed" row is the cost requirement. The AD pass must not
walk the detached sub-graph, so its size does not change the size of what
the pass produces.

### 2.3 Representation

A dedicated signal node, `Detach(x)`, distinct from `x` by hash-consing. A
stop *set* holding `x` itself would be wrong: if `x` is also used on a path
that is not detached, putting `x` in a stop set would cut the gradient on
that path too.

The node is erased at the end of propagation, after every `fad`/`rad` has
been processed. The exception is the inside of `BlockReverseAD` bodies,
which are lowered in `transform`. There it stays until the block sweep, so
every walker that enters such a body must accept it; the coverage test
`crates/transform/src/signal_fir/tests/coverage.rs` must name it.

### 2.4 Surface

- lexer and parser rule;
- `boxes` builder, matcher and printer;
- `eval` arity;
- `propagate`: the box lowering, `forward_ad.rs`, `reverse_ad.rs`, and the
  erasure;
- `signals`: tag, `SigMatch::Detach`, builder, dump;
- `transform`: the `BlockReverseAD` body walkers;
- `draw`;
- docs: `docs/fad-note-en.md`, `docs/rad-note-en.md`,
  `docs/fad-rad-synthesis-{en,fr}.md`, and the primitives list in
  `docs/README.md`.

This is much smaller than a clocked kind (§3 of the `repeat` plan): once
erased, the node never reaches `normalize`, `sigtype` or the backends.

### 2.5 Checks written first, and their mutations

1. **The primal is bit-identical.** `faustprobe --compare` at tolerance 0
   between a program and the same program with a `detach` inserted.
   Mutation: lower `Detach` to `x * 1.0` in the primal path.
2. **Exact zero tangents.** `fad` and `rad` over `x * detach(x)` give `x`,
   not `2x`, on the interpreter and on Cranelift; there is one symbolic case
   and one `BlockReverseAD` case (a recursive body). Mutation: make
   `forward_ad.rs` treat `Detach` as transparent.
3. **Cost independence.** A structural test counts the signal nodes created
   by `fad` around a detached sub-graph of growing size (a chain of 10, 100
   and 1000 operations). The count must not grow. Mutation: transform `x` and
   then zero the tangent.
4. **Nesting.** `fad(fad(f(detach(x), y), y), y)` against finite
   differences.
5. **The faust-diff-demo shape**, reduced to a corpus fixture (leader seeded
   on a detached scout). It compiles, and its output equals the cast
   version's output within the cast's quantization.

### 2.6 Decisions before A starts

- **D-A1, the name.** `detach` (PyTorch, and the name already used in §6.2
  and in faust-diff-demo's note) or `nograd`. Both are free (§1.1).
  Recommended: `detach`.
- **D-A2, one keyword or two.** `detach` is `withgrad(_, (!, 0))` (§3.3).
  If B is certain to follow, the cheaper end state is one keyword,
  `withgrad`, with `detach` a library definition. If B is uncertain, `detach`
  is worth its keyword alone. Recommended: implement `detach` as its own
  primitive now (it is small, fully specified, and asked for), and decide at
  B whether `withgrad` replaces its implementation (both stay valid
  spellings either way).

## 3. B — `withgrad(F, G)`, a derivative given by the program

### 3.1 The need

Three cases where today's derivative is missing, wrong for the purpose, or
an error:

- **A foreign function outside the recognized set.** `fad` returns a zero
  tangent, `rad` refuses (§4.2). The user has no way to supply the
  derivative.
- **Tables.** A read-only table index is differentiated by a symmetric
  finite difference (DIFF-SRC-001). That is a model chosen by the compiler.
  A program that stores a function and its derivative in two tables cannot
  say so.
- **Quantizers.** The straight-through estimator (derivative 1 through a
  rounding) is the standard way to train through a quantizer. Today the
  integer cast gives 0. `docs/rad-note-en.md` records that the block sweep
  once returned a straight-through value by accident and was corrected to 0.

### 3.2 Semantics

`withgrad(F, G)`, with `F : n → 1` and `G : n → n`, has the arity of `F`.
`G` receives the same `n` inputs and returns the partial derivatives
`∂F/∂xᵢ`.

| context | effect |
|---|---|
| value | `F(x)` |
| `fad` | tangent `Σᵢ Gᵢ(x) · ẋᵢ`; `F` is not differentiated, so not walked |
| `rad` | adjoint `x̄ᵢ += ȳ · Gᵢ(x)` |
| nested | the outer differentiation applies the same rule to `F`, and differentiates `G` normally (the `custom_jvp` model of JAX) |
| outside AD | `F(x)`; `G` is dead and erased with the node |

One specification serves both modes: from the partials, the forward rule is
a dot product and the reverse rule a scaling. Invariant inherited from A: a
partial that folds to a literal zero (`DerivativeZeroFolder`) does not
trigger the transformation of its argument, so `withgrad(_, (!, 0))` costs
what `detach` costs.

### 3.3 Examples

```faust
cbrt = ffunction(float cbrtf|cbrt|cbrtl (float), <math.h>, "");
cbrt_d = withgrad(cbrt, \(x).(1.0 / (3.0 * cbrt(x) * cbrt(x))));

round_ste = withgrad(rint, (!, 1));   // straight-through
detach    = withgrad(_, (!, 0));      // A as a derived form

lut(i) = withgrad(\(i).(rdtable(N, T, int(i))),
                  \(i).(rdtable(N, dT, int(i))))(i);
```

### 3.4 Representation and surface

A signal node `WithGrad(f, partials, args)`. It carries the primal signal,
the list of partial signals, and the argument signals the chain rule needs.
It is erased like `Detach`, with the same `BlockReverseAD` exception. Its
surface is A's, plus:

- the chain-rule sum in the three AD sites;
- the contribution formula placed in `signals::ad_rules`, so that the
  symbolic and block sweeps share it rather than adding a fourth copy (see
  §4.1 for the three that exist).

### 3.5 Checks written first

1. Finite differences agree with `fad` and with `rad` when `G` is the true
   derivative (`cbrt`, a two-argument `hypot`). This is the independent
   check.
2. `fad`/`rad` agreement, and `-O0` against the maximum optimization level.
3. Straight-through: `fad(round_ste(x), x)` gives `1`.
4. Nesting: the second derivative of `cbrt_d` against finite differences of
   the first.
5. Cost: no transformation of `F`'s sub-graph (A's node-count test).

Mutations: drop one term of the dot product; transform `F` as well as
applying the rule.

### 3.6 Decisions

- **D-B1, the name.** `withgrad`, `customgrad`, `gradwith`; all free.
  Recommended: `withgrad`.
- **D-B2, state in `F` and `G`.** The pointwise rule is only the derivative
  of a memoryless `F`. Choice: reject delays, recursions, table writes and
  clocked blocks in `F` and `G` (new `FRS-PROP-*` error), or accept them
  "as the program's model". Recommended: reject in the first version;
  read-only tables allowed.
- **D-B3, several outputs.** `F : n → m` needs `G : n → m·n`, a Jacobian in
  row order. This is needed to give `nffunction` (D) a derivative.
  Recommended: `m = 1` first, the general form when D lands.

## 4. C — the math functions the libraries declare as `ffunction`

### 4.1 What the compiler knows today, and where

The standard libraries declare 25 C functions with `ffunction`, nearly all
in `maths.lib`:

- hyperbolic and inverse hyperbolic: `sinh`, `cosh`, `tanh`, `asinh`,
  `acosh`, `atanh`;
- other real functions: `cbrt`, `hypot`, `erf`, `erfc`, `expm1`, `log1p`,
  `copysign`, `tgamma`, `lgamma`;
- floating-point manipulation: `ldexp`, `scalbn`, `logb`, `ilogb`,
  `nextafter`;
- tests: `isnan`, `isinf`;
- Bessel: `j0`/`j1`/`jn`/`y0`/`y1`/`yn`;
- `arc4random` (in `noises.lib`).

The compiler has no node for any of them. It recognizes some of them by
matching the symbol name against string lists, in fourteen files outside
test modules:

- `propagate`: `forward_ad.rs` (`transform_ffun`), `reverse_ad.rs`
  (`RAD_FFUN_UNARY_NAMES`, `FFunUnaryKind`);
- `transform`: `signal_fir/module/bra.rs` (`propagate_bra_ffun_adj`);
- `fir`: `schedule/effects.rs`, `schedule/check.rs` (the pure-function
  lists of the scheduler);
- `codegen`: each backend's own table (wasm, interp, cranelift `host.rs`,
  julia, cmajor, asc, codebox ×2, rust).

The JIT backends also call `foreign-call`'s `libm` bridge, which links
`cbrt`, `hypot`, `erf`, … from the system library.

Consequences:

- **Three copies of the hyperbolic derivatives**: FAD, symbolic RAD, block
  RAD. None of them is in `signals::ad_rules`, which exists so that the
  symbolic and block sweeps share their formulas.
- **No interval.** `sigtype`'s `infer_foreign_fun_type` gives every
  `FFun` the default interval, as the C++ reference does. Yet the `interval`
  crate implements `sinh`, `cosh`, `tanh`, `asinh`, `acosh` and `atanh`. It
  has nothing for `cbrt`, `hypot` or `erf`.
- **AD covers only the six hyperbolics** (§4.2).
- **WASM.** The backend imports the hyperbolics, `isnan` and `copysign`
  under the runtime's math names (`_tanh`, …). It imports `cbrt` under its
  C symbol, `env.cbrtf` (measured). `foreign-call` has no `libm` on wasm32.

### 4.2 Measured on 2026-10-27

| program | faust-rs | C++ 2.84.3 |
|---|---|---|
| `fad(tanh(x), x)` at `x = 0.5` | `0.7864477` = 1 − tanh² | — |
| `fad(cbrt(x), x)` at `x = 8` | **`0.0`**, no diagnostic (true value 1/12) | — |
| `rad(cbrt(x), x)` | `FRS-PROP-0001` "rad cannot differentiate … (ffun)", help: "or use fad(...) which has the same coverage" | — |
| `_ @ int((1 + tanh(x)) * 100)` | `FRS-COMP-0004`, delay amount unbounded | rejected, "possible negative values" |
| `_ @ int((1 + sin(x)) * 100)` | compiles | compiles |

The FAD zero is documented in `docs/fad-note-en.md` as a "zero tangent"
boundary ("no implemented forward rule"). It is still a silent wrong
gradient for a function that has a derivative everywhere.

### 4.3 Options

- **C1, one classifier table, `FFun` kept (recommended).** One table maps
  a symbol triplet (and, per D-C2, its header and signature) to a
  `KnownFn` and its properties: arity, interval rule, derivative rule, and
  "piecewise constant" for `isnan`, `isinf`, `ilogb`, …. Every site of §4.1
  asks the table instead of matching strings. The signal graph, `normalize`,
  the FIR and the emitted calls are unchanged.
- **C2, one generic native node `XMath(op, args)`.** The operation is stored
  in the node. This gives more freedom (folding, backend intrinsics without
  a header). It touches every exhaustive match, and the emitted code
  changes.
- **C3, one variant per function, like `Exp10`.** `Exp10` alone occupies 58
  lines (tests included) in 15 crates; twelve more functions would add
  hundreds of match arms. Rejected.

C1 delivers everything §4.1 lacks except constant folding, and the move to
C2 later stays open.

### 4.4 Phases

**C-1, the table, same output.**

- Producer: the table, and every site of §4.1 rewritten to use it.
- Check written first: the codegen-identity differential, base against
  new, over every `.dsp` of `tests/` and `examples/`, on the C++, C, Rust,
  WASM and interpreter backends.
- Mutation: drop `tanhl` from the table; the differential must flag the
  long-double programs.
- No registry entry: no behaviour change.

**C-2, intervals.**

- Producer: interval rules for the table's functions. The hyperbolics come
  from the `interval` crate. Rules for `cbrt`, `hypot`, `erf`, `erfc`,
  `expm1`, `log1p` and `copysign` are added to it, documented as Rust-only
  provenance. `isnan` and `isinf` are boolean.
- Independent check: each rule against dense sampling of the function on
  random input intervals, endpoints and special values included
  (containment).
- Mutation: compute `erfc`'s interval as `[erfc(lo), erfc(hi)]`, without
  swapping the bounds of a decreasing function.
- This is an extension: programs the reference rejects now compile
  (`@(int((1 + tanh(x)) * 100))`), and buffer sizes may change where an
  interval decides them. The codegen-identity differential lists every
  changed output, and each change is explained in the commit.
  `DIFF-BEH-029` is added.

**C-3, derivative rules.**

- Producer: rules in `signals::ad_rules` for:
  - the six hyperbolics, moved there from the three sites, which drops two
    copies;
  - `cbrt`, `hypot` (binary), `erf`, `erfc`, `expm1`, `log1p` and
    `copysign` (piecewise);
  - an explicit zero for the piecewise-constant functions.
- Independent check: finite differences (Richardson) for each function, in
  FAD, symbolic RAD and `BlockReverseAD`.
- Mutation: the sign of `erfc'`.
- Fix the RAD help text of `FRS-PROP-0001` in the same change.

**C-4, optional: backends by `KnownFn`.** The backend tables become maps
from `KnownFn` to intrinsic. This overlaps the codegen factorization stream
and is decided there.

**C-5, optional: WASM for `cbrt`, `hypot`, `expm1`, `log1p`.** JS `Math`
has them, but not `erf`. This changes the import contract with the WASM
runtime, so the runtime owners decide.

### 4.5 Decisions

- **D-C1, where the table lives.** It is needed by `signals`, `sigtype`,
  `propagate`, `transform`, `fir` and `codegen`; their only common
  dependency is `tlib`, which is the wrong home. Options: a new leaf crate
  (`mathfn`, no dependencies), or a table in `signals` plus one in `fir`
  checked equal by a test. Recommended: the leaf crate (one table, no
  equality test to maintain); it widens the public API baseline by one crate.
- **D-C2, the key.** Today only the name counts:
  `ffunction(float tanh(float), <mine.h>, "")` is treated as `<math.h>`'s
  `tanh`. Options:
  - name only (current behaviour);
  - name + signature;
  - name + signature + `<math.h>`.

  Recommended: name + signature. The signature (arity, number types) is
  what the interval and derivative rules depend on. A header test would
  make a program that declares the same function through another header
  lose its intervals without any message.
- **D-C3, compile-time folding of `tanh(0.5)`.** The reference does not
  fold `FFun`. Folding would replace the runtime `libm` result by Rust's,
  which can differ in the last bit, and would change the emitted text.
  Recommended: no folding in this stream.
- **D-C4, the policy for a still-unknown `ffunction` under `fad`.** Once C-3
  lands, "unknown" means a user's own function, Bessel, gamma, or
  `arc4random`. Options: keep the zero; warn; reject as `rad` does.
  Recommended: reject in both modes, with a hint to `withgrad`, landing
  with B so that the error has a way out. This is an error-policy decision
  (AGENTS §12).

## 5. D — `nffunction`

The proposal is complete as a design. What remains open, by its own §7, is
the validation of the syntax, the ABI and the backend scope before any
implementation. This plan adds three points:

- **Name**: free (§1.1).
- **AD**: an `nffunction` call falls under the `FFun` policy of D-C4. Its
  derivative needs `withgrad` with several outputs (D-B3). Without it, a
  frame FFT built on `nffunction` cannot be trained through, where the pure
  Faust `fft_framed` can.
- **Order**: after A–C. Its value is frame-rate spectral processing at
  large N. `interleave.lib` and `fft_framed` already cover that in pure
  Faust, with the compile-time limits the
  [FFT-scalability note](fft-scalability-cse-in-clocked-blocks-2026-07-09-en.md)
  measures.
  Starting it is a decision about those limits, not about a missing
  capability.

## 6. E — `repeat`

The `repeat` plan's own status (§8) says the library form reproduces the
semantics, and the primitive is kept "as a design for a code-generation
improvement, not as a need". Nothing measured since changes that.

**The name.** D1 of that plan chose `repeat` because the libraries had no
identifier of that name. They do: `signals.lib` defines

```faust
repeat(1, FX) = FX;
repeat(n, FX) = FX <: si.bus(N), repeat(n-1, FX) :> si.bus(N)
```

since faustlibraries `9aa3e8f8` (2023-10-11, "add si.repeat"). The
definition is present in the `signals.lib` of the pinned C++ checkout and
of upstream 2.90. As a keyword, `repeat` would break the
parse of `signals.lib`, and with it of every program that imports
`stdfaust.lib`. The library form W4 is not affected: a definition in a
faust-rs library is qualified by its prefix and reserves nothing.

So, if the primitive is ever implemented, D1 reopens. `iterate`, `until`
and `whilst` must pass the check of §1.1 again; `loop` was excluded for its
24 uses. The plan's §8 records the correction on this date.

## 7. Considered and not retained

- **A frame or vector signal type.** `interleave.lib` builds frame-rate
  processing on boolean `ondemand` "without any new compiler primitive"
  (the `interleave` note, §1), and `fft_framed` reuses `analyzers.lib`
  unchanged.
- **A native `selectn`.** `ba.selectn` builds a tree of `select2`. A switch
  node would only change the emitted code, and no measurement shows a cost.
- **Native nodes for `log2`, `exp2`, and `maths.lib`'s `log1p`/`expm1`
  written as formulas.** They are expressions over existing primitives and
  already have intervals, folding and derivatives. Only the functions
  declared with `ffunction` lack them, which is C's scope.
- **Polyphony, instance identity, random-seed primitives.** No program or
  note asks for them.

## 8. Order, deliverables, pass criteria

| step | deliverable | passes when |
|---|---|---|
| 0 | answers to D-A1, D-A2, D-B1–B3, D-C1–C4 | recorded in this document |
| 1 | A: `detach` | checks §2.5 green, mutations rejected, `DIFF-SRC-005`, notes updated |
| 2 | C-1: classifier | codegen-identity differential empty |
| 3 | C-2: intervals | sampling check green; every changed output explained; `DIFF-BEH-029` |
| 4 | C-3: derivative rules | finite differences green in the three AD sites; copies removed; RAD help fixed |
| 5 | B: `withgrad` (+ D-C4) | checks §3.5 green; `DIFF-SRC-006`; `detach` re-expressed per D-A2 |
| 6 | D: `nffunction` | its own §8 |
| 7 | E: `repeat` primitive | a measured codegen need, and a name that passes §1.1 |

Steps 2–4 do not depend on step 1 and can run in parallel with it. Step 5
depends on 1 (shared erasure and `BlockReverseAD` handling) and on 4 (the
shared rule site). The identifiers `DIFF-SRC-005`, `-006` and
`DIFF-BEH-029` are the next free numbers on the base commit and are
assigned when each step lands.
