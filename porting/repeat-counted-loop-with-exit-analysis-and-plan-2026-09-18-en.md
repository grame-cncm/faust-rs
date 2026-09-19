# `repeat`: a counted clock loop whose exit is decided inside. Analysis and plan

Date: 2026-09-18

Status: **the semantics is reproducible with the existing wrappers** (§1.5,
measured the same day); the compiler primitive of §2 is kept as a design for
a code-generation improvement, not as a need. §8 records what landed.

Renamed 2026-09-19: the primitive is `repeat`. The version of 2026-09-18
called it `iterate`, a name the journal entries and the commit messages of
that day keep; the file was renamed with it. The semantics, without the
implementation, is the subject of `docs/repeat-note-en.md` (and `-fr.md`).

## Scope

An `ondemand` block with an integer clock runs its body `H` times per outer
sample; the transform emits a counted loop, `for (lOd = 0; lOd < H; lOd++)`,
with `H` lowered once in the outer region (`signal_fir/module/clocked.rs`,
`GuardShape::CountedLoop`, `simple_for_loop` in `ensure_guarded_block`).
Section 2.5 of `libraries/optimizers-overview-en.md` showed what that gives
to differentiable DSP: a loop with state at run time, and in particular an
implicit solver (Newton on `y = tanh(x - fb y)`) whose number of steps is
decided per sample. The question asked the same day: would generating a
`while` rather than a `for` be more flexible, or more powerful?

This document answers it (§1), shows that the answer, "a counted loop with
an exit flag", is already expressible with the wrappers that exist, as a
library function (§1.5), and keeps the design of a compiler primitive (§2),
its surface (§3) and its phases (§4, §5) for the day the remaining
difference, code generation, is worth it. Every number
quoted was measured on the release binaries of `5439e0c2` with
`faustprobe`, double precision, 48 kHz; the programs are in the journal
entry of the day and quoted here where they matter.

## 1. Analysis

### 1.1 What `for` and `while` mean here

The difference is not syntactic. With `for`, the count is a signal of the
*outer* domain, evaluated once before the loop. With `while`, the exit is
evaluated *inside* the loop, on the iterate. Faust's model gives every
signal one domain, and the clock of a wrapper is an outer signal (the first
input of the wrapped box); the iterate lives inside. So "a while" means a
condition computed in the inner domain reaching the control structure,
which no existing node does.

### 1.2 What the counted loop already does, measured

Section 2.5 of the overview gives the two-layer form: outside, the residual
of the warm start `F(x, y_prev)`, computable before the block runs (the new
input, the held solution), decides the count, 0 when the previous solution
already satisfies the tolerance and a budget `Kmax` otherwise; inside, a
nested boolean `ondemand` takes one step per iteration while the residual
of the current iterate is above the tolerance:

```faust
body(xi) = inner ~ _
with { inner(y) = ((abs(F(xi, y)) > tol), y) : ondemand(\(yp).(step(xi, yp))); };
solve(x) = sel ~ _
with { sel(yp) = ((abs(F(x, yp)) > tol) * Kmax, x) : ondemand(body); };
```

With `tol` 1e-12 and `Kmax` 8, against the unrolled `newton(8, ...)` from 0
(10.2 ms per second of audio), equal to it to 1.1e-12:

| input | steps per sample, mean and largest | cost per second of audio |
|---|---|---|
| constant | 0 after the first sample | 0.32 ms |
| sine, 100 Hz | 2.4, 3 | 5.9 ms |
| sine, 2 kHz | 3.0, 3 | 6.5 ms |
| white noise | 3.5, 5 | 7.4 ms |

What the unused iterations of the budget cost: on the 100 Hz sine, budget
8 against budget 3 (the largest count that input needs), 10.7 and 10.4 ms
against 10.3 and 9.2 ms per two seconds of audio in two runs each, between
4 and 12 %, the measurement's own spread being of that order. And a budget
too small is wrong, not slow: on white noise with budget 3, the samples
that needed 4 or 5 steps are left unconverged (first difference at frame
480, 1e-8 relative).

### 1.3 More powerful? No. More flexible? Yes

**Not more powerful.** In real time a `while` without a bound is a block
whose worst-case time is unbounded, and an iteration that does not converge
is an audio thread that never returns. So it is `while (cond && n < Kmax)`,
which is exactly the counted loop with an early exit, which the two-layer
form computes. Given the bound, the same functions are expressible today.
The counted loop also gives the type system something a `while` would lose:
the clock's interval selects the shape (range within [0, 1]: an `if`;
integer: a counted loop) and bounds the iteration count statically.

**More flexible, in three ways.** The exit reads the actual iterate, not
the residual of the warm start, which only decides the first iteration. One
layer instead of an outer residual, an inner gate and a budget. And the
condition can be anything computed inside: a residual, a step that became
too small, a loss that stopped decreasing, the sub-stepping of an adaptive
integrator (RK45 with step rejection on a stiff circuit, where the number
of sub-steps is known only inside). A rational resampler, by contrast,
stays a `for`: its count is read off the phase accumulator before the loop.

**What it saves**: the 4 to 12 % of §1.2, and the double layer. What it
does not save is the bound.

### 1.4 The form

Not a `while`, but **a counted loop with an exit flag**:

```c
for (lOd = 0; lOd < H; lOd++) { body; if (stop) break; }
```

The clock stays the bound, so interval typing and the worst-case block time
keep their meaning; the body decides the exit through one distinguished
output, the way `op.gated` uses the last output of its block as a flag. The
FIR has no `break`; the loop is a `WhileLoop` on `(lOd < H) & (iStop == 0)`
with `lOd` and `iStop` explicit stack variables (§2.3).

### 1.5 The same thing with the wrappers that exist

Asked next: is the semantics of §1.4 really out of reach of `ondemand`
alone? It is not. An outer integer `ondemand` runs `H` times per sample;
inside it, a boolean `ondemand` runs the body on the first iteration of the
sample and then while the flag the body produced at the previous iteration
is non-zero. The body's outputs hold between its fires, so the held flag is
what the next iteration reads, and the skipped iterations cost an `or` and
an `if`. The first iteration of a sample is where the sample index, passed
in as an input, differs from its value at the previous iteration in local
time; no feedback from outside is needed. For any arity:

```faust
// C : n -> m + 1, its last output the continue flag; repeat(C) : n + 1 -> m
repeat(C) = (_, (ba.time + 1, si.bus(n))) : ondemand(shell)
with {
    n = inputs(C);
    m = outputs(C) - 1;
    shell = (first, si.bus(n)) : (gate ~ (si.block(m), _)) : (si.bus(m), !)
    with {
        first = _ <: (_ != _');
        gate = ((_, _) : |), si.bus(n) : ondemand(C);
    };
};
```

Checked on a body that counts its executed iterations: a flag always 1 and
`H = 5` count 5 per sample; a flag always 0 count exactly 1 per sample (the
do-while), including with `H` changing from one sample to the next; a flag
that falls after 3 count 3 with `H = 5` and are cut at 2 with `H = 2`;
`H = 0` holds. The Newton solver of §1.2 written with it, the body one step
on its own state, the flag the residual of the new iterate, the outer skip
kept: equal to the unrolled solver to 1.1e-12, the same steps per sample as
the two-layer form (2.4, 3.0, 3.5 on the three moving inputs) and cheaper
than it, since the skipped iterations no longer recompute the residual:

| input | two-layer form of §1.2 | library `repeat` |
|---|---|---|
| constant | 0.32 ms | 0.25 ms |
| sine, 100 Hz | 5.9 ms | 5.3 ms |
| sine, 2 kHz | 6.5 ms | 6.1 ms |
| white noise | 7.4 ms | 7.3 ms |

What the library form does that the primitive would do: everything of
§2.1, including local time counting executed iterations (the body lives in
the inner block) and the exit read on the iterate. What it does not: the
code. Two nested guards inside a counted loop instead of one `while`, the
`or`, the `if` and the held flag on every skipped iteration; the figures
above say what that costs, which is the measurement's spread.

One thing it uncovered. `fad` around it failed in the clock-environment
inference (`FRS-SFIR-0008`, "block env must be an ancestor of value env
nil"): the flag is a held lane no seed reaches, its tangent a literal
zero, and `transform_seq` sequenced that literal through the augmented
block (`Seq(od_aug, 0)`), an audio-rate value under a block environment,
which survived inside the recursion group feeding the flag back. The
literal now stays a literal (`forward_ad.rs`, commit of the day, with the
nested-flag shape as its test); the gradient of the solver through the
library `repeat` then equals the central difference (§8).

So the plan below changes status: **W4 first**, the library function and
its documentation, on the compiler as it is; W1 to W3 only if the code
generation of a `while` with a static bound is wanted for its own sake, the
measured difference being under the spread.

## 2. Design

### 2.1 The primitive

A fourth clocked wrapper, next to `ondemand`, `upsampling`, `downsampling`:

```faust
repeat(C)
```

**Arity.** If `C : u → v+1` then `repeat(C) : u+1 → v`. The extra first
input is the clock `H`, as for the three others; the last output of `C` is
the *continue* flag, consumed by the primitive, not exposed. `C` must have
at least one output besides the flag (`v ≥ 1`); a body with the flag alone
is an error, as a `gated` block without an output besides its flag.

**Semantics.** Per outer sample:

| clock `H` | effect |
|---|---|
| range within [0, 1] | as `ondemand`: the body runs once if `H ≠ 0`, the flag is read but cannot matter |
| integer range wider than [0, 1] | the body runs, then again while its flag was non-zero, at most `H` times |
| real | cast to int first, as the normal form does for the three wrappers and as the C++ reference does: `⌊H⌋` iterations, so a clock of 0.5 never runs the body and 2.5 runs it twice (measured on both compilers, 2026-09-18; the `docs/ondemand-note` said otherwise and is corrected) |
| constant 0 | the outputs are 0, the body never runs (as `ondemand`) |
| constant 1 | the body inlined once, its flag output dropped (as `ondemand`'s `H == 1` collapse) |

The flag is evaluated **after** the iteration's outputs: the held outputs
are those of the iteration that raised it, or of the last one when none
did. The first iteration always runs when `H > 0` (a do-while); skipping
the block entirely remains `H = 0`, decided outside on the held state, as
in §1.2. Inputs are snapshotted at the block's start and constant across
iterations, as for integer `ondemand` (no zero-stuffing). Local time counts
executed iterations: a `~` recursion or a delay in the body advances once
per executed iteration, the per-domain cursor `fIOTA_d<i>` too. Nested
wrappers of any kind are allowed; an exit leaves its own loop only.

**Not for `upsampling`.** Its input arrives on the last iteration
(`us == H - 1`, the zero-stuffing contract); an early exit would never
deliver it. **Not for `downsampling`**, which runs once. The flag polarity
is *continue* (non-zero: go on), reading like `while (cond)`; the
alternative, a *stop* flag with a name such as `until`, is decision D1.

### 2.2 The signal node

Propagation (`propagate/src/engine.rs`, `propagate_clocked_wrapper`) builds
today `SIGOD(Clocked(env, clock), PermVar(Clocked(env, y_i))...)` with a
fresh `ClockDomainKind`. The new node is a new tag, `SIGREPEAT`, with a new
`ClockDomainKind::Repeat` and `ClockedWrapperKind::Repeat`:

```
SIGREPEAT(Clocked(env, clock), PermVar(Clocked(env, y_1)), …, PermVar(Clocked(env, y_v)), Clocked(env, flag))
```

The flag is the last lane, a `Clocked` payload **without** a `PermVar`: it
is computed in the block and not held. A distinct tag rather than a
convention on the last lane of `SIGOD`: the lowering takes every payload
lane of `SIGOD` after the clock for a hold (`declare_hold_fields` refuses
anything else, and 2026-09-18's `FRS-SFIR-0007` on a zero tangent lane is
what a lane-shape convention costs), and the type, the dumps and the
checkers should name what they see.

`sigtype` types the flag like a clock (any nature; the loop tests `!= 0`)
and the node like `SIGOD` (real, `Samp`, never `Konst`). The result memo
admits it as the three others (same key, same block). Constant clocks fold
as in the table above.

### 2.3 The lowering

`decode_clocked_wrapper`: for `Repeat`, the holds are all lanes but the
last, the flag is the last. `select_guard_shape`: a fourth shape,
`CountedLoopWithExit`, for an integer non-boolean clock; `BoolIf` for a
boolean one (the flag lowered and discarded, or not lowered at all: it
cannot matter). `emit_guard_precondition` declares `int lOd = 0; int iStop
= 0;` in the outer region. `lower_guarded_body` lowers the flag payload
with `lower_clocked_payload` after the hold stores and appends `iStop =
(flag != 0)` to the iteration's immediate phase, after the stores, before
the sample-end phase (cursor bump, `IfWrapping` advances); `lOd = lOd + 1`
closes the sample-end phase. `ensure_guarded_block` wraps the body in
`WhileLoop((lOd < H) & (iStop == 0), body)` under the existing `if (H !=
0)` guard. The shape of the generated C++ for the solver of §1.2 written
with `repeat` (a sketch of what W2 must emit, not an emission):

```c++
if (iSlow0 != 0) {
    int lOd0 = 0; int iStop0 = 0;
    while ((lOd0 < iSlow0) & (iStop0 == 0)) {
        fPerm0 = fPerm0 - F(fTemp0, fPerm0) / dF(fTemp0, fPerm0);
        iStop0 = (std::fabs(F(fTemp0, fPerm0)) > 1e-12);
        lOd0 = lOd0 + 1;
    }
}
```

The FIR verifier already checks a `WhileLoop`'s condition (FIR-L03).

### 2.4 Backends

`WhileLoop` exists in the FIR (`fir/src/builder.rs`, C++ parity
`WhileLoopInst`), is lowered by Cranelift (`lower_while_loop`) and WASM,
emitted by the C family (C, C++), Rust, Julia, Codebox, Cmajor and ASC.
Nothing produces one today: only the CSE pass rebuilds one it met. Two
consequences: those emitters have never run on a real program and get
their first fixtures here; and the **interpreter has no `WhileLoop`**
(nothing in `codegen/src/backends/interp` names it). Its `Loop` opcode is a
counted loop whose exit test sits at the end of the body; a `WhileLoop`
needs either a new opcode or that pair with the condition evaluated first
(decision D3).

### 2.5 Differentiation

`fad` around the block (`forward_ad.rs`, `augment_block`) interleaves each
held lane's primal and tangents; the flag lane gets its **primal only**.
Its tangent is zero anyway when it is a comparison, but a real-valued flag
(`r - tol`) would otherwise put a tangent lane where the lowering expects
the flag. `transform_seq` is unchanged: `Seq` consumers read holds, never
the flag. The correctness is the one stated in the overview: at the exit
the tangent of a contractive iteration is the derivative of its fixed point
(Christianson 1994), so a block that exits on convergence gives the implicit
derivative. `rad` across the boundary stays rejected, with the fourth kind
named in the message (`reverse_ad.rs`, `stateful_rad.rs`).

### 2.6 Outside the design

Vector mode **does** lower clocked blocks, contrary to what this document
first said: `-vec` and `-ss` accept the three wrappers and emit a counted
`vclock_d<i>_fire` loop inside the vector loop, modelled by the `clock_ad`
checkers (checked 2026-09-18 on `upsampling`, `downsampling` and integer
`ondemand` programs). The fourth kind needs that lowering and that model
too: `vector/lower/signal.rs` and `vector/clock_ad/{model,build,check}.rs`
of the surface (§3) are code to write, not arms to extend. No C++
counterpart: the C++
reference branch for clocked wrappers (`8eebea429`) has no such primitive,
so no C++ differential, as for `fad`; the mirror points for a later C++
port are `boxOndemand` and `propagate.cpp`'s clock environment, and
`generateOD` in `compile_scal.cpp`.

### 2.7 `repeat` as the general form

Asked after §1.5: if `repeat` were the primitive, would the three others
be instances of it? They are, up to two orthogonal features, and this was
measured:

- **`ondemand`**, both readings: `repeat` with a flag constantly 1. The
  clock is cast to int by the normal form (`promote_clocked_family`), so
  the body runs `⌊H⌋` times whatever the clock's range; the `if` of a range
  within [0, 1] is the emission of the case `⌊H⌋ ∈ {0, 1}`, an
  optimization, not a second semantics. (The `select_guard_shape` refusal of
  a real non-boolean clock is unreachable after that cast.)
- **`downsampling`**: `ondemand` with a divided clock computed in the parent
  domain, `((+(1) ~ _) - 1) % H == 0`: identical sample for sample to
  `downsampling(3)` on white noise. The per-domain `fDSCounter` of the
  emission is that counter as state rather than as a signal.
- **`upsampling`**: integer `ondemand` whose body zero-stuffs its input,
  `x * (i == H - 1)` with `i` the in-sample iteration index: identical
  sample for sample to `upsampling(3)`. The `ZeroPad` node of propagation is
  that product done once by the compiler.

What none of the four express and what the two rate wrappers add is the
**sample rate seen inside** (`SR * H` in `upsampling`, `SR / H` in
`downsampling`, the `sample_rate` of the clock environment in
`make_clock_env`): a property of the domain, not of the loop, which a body
reading `ma.SR` depends on and which no wrapper written in the library can
set. So the layering is: one loop primitive with a bound and a flag; the
zero-stuffing of inputs as a body-level rewrite (library or compiler); the
rate substitution as a domain property; the three keywords as derived
forms, kept for the programs and the C++ that have them. A compiler that
took `repeat` as its one guard shape would emit the `if`, the counted loop
and the modulo as optimizations of the bounded `while` when the flag is
constant and the bound has the right range, and the vector-mode checkers
would model one kind. That is the design of §2 seen from the other end; it
does not change its surface (§3) or its order (§4).

### 2.8 Decisions

- **D1, name and polarity.** The name is `repeat` (decided 2026-09-19;
  the first version of this document said `iterate`). `loop` was out: 24
  uses as an identifier in the libraries; `iterate`, `until`, `repeat`,
  `whilst` have none. The polarity stays open. The last output of the body
  can mean *continue* (non-zero: run again) or *stop* (non-zero: leave).
  The model of §2.1 is the same either way; what changes is the sense of
  one bit, and it changes it in every program: a body whose flag is
  `abs(F) > tol` under the continue reading must become `abs(F) <= tol`
  under the stop reading, and a program written for one and compiled under
  the other runs one iteration where it should run to convergence, or the
  whole budget where it should stop at once. For *continue*: it is what the
  library form of §1.5 implements and measures, and it follows the
  convention of `op.gated`, whose last output is a gate with 1 meaning
  "active". For *stop*: the name reads `repeat … until`, and a convergence
  test is naturally a stop condition ("converged, so stop"). The decision
  has to be taken before the first program is written (W4), and cannot be
  revisited afterwards without changing the meaning of every program;
  §8 of `docs/repeat-note-en.md` states it the same way.
- **D2, minimal body.** `v ≥ 1` held output besides the flag (proposed), or
  allow a flag-only body (a loop with state and no output has no observable
  effect in Faust; reject).
- **D3, the interpreter.** A `While` opcode, or `Loop` + `CondBranch` with
  the condition evaluated before the body.
- **D4, cross-backend evidence.** The numeric tests run on the interpreter
  and on Cranelift; the C, C++ and Rust emissions are compiled and run by
  the impulse runner only for fixtures with a C++ oracle, which these
  cannot have. Either a runner mode accepting a Cranelift-produced
  reference for `rust-only` fixtures, or the FIR verifier plus the three
  emitters' structural tests as the evidence for those backends.

### 2.9 Changing the emitted code, if the semantics stays the same

Asked next: the emitted code may change, provided the semantics does not.
What that permits depends on what pins the semantics, and on whether the
pin is on samples or on text. Counted on 2026-09-18:

**Sample-level, independent of the text.** The impulse tests: 133
programs, of which 39 use a clocked wrapper (21 `ondemand`, 9
`upsampling`, 9 `downsampling`), against the pinned C++ reference
`8eebea429`, on eight backends, the vector variants inheriting the gates
(README, sweep of 2026-08-14: 133 matches on every backend). The 14
differential tests of `cpp_clocked_differential.rs`, against the same
binary. The numeric tests of the compiler crate on clocked programs: 41 in
`ondemand_pipeline.rs`, 32 in `optimizers_lib.rs`, 14 in
`ddsp_examples.rs`, 2 in `interleave_fft.rs`, 1 in
`clocked_waveform_regression.rs`. The `faustprobe` output snapshots, whose
corpus mode renders the 133 impulse programs on Cranelift, byte-exact on
the platform of record.

**Text-level.** One golden snapshot (the only clocked corpus fixture
eligible for golden), and the 8 tests of `clocked_emission_structure.rs`
with their 14 assertions on `if (`, `for (int lOd`, `fDSCounter`,
`fIOTA_d`: they document the shape, they do not hold the semantics.

So the decision is well founded: a change of shape is checked by oracles
that compare samples, on every backend, before and after, and the
text-level tests are rewritten as documentation of the new shape. Three
conditions make the check real rather than assumed:

1. **The references exist locally before the change.** This machine holds
   94 of the 133 `.ir` references; the 39 clocked ones are not among them.
   `make reference` against the C++ checkout regenerates them; the sweep
   is then run before the change (must be 133 on every backend, as on
   2026-08-14) and after.
2. **Every backend, and the vector variants.** The `WhileLoop` emitters
   have never run on a real program (§2.4) and the interpreter has none;
   the sweep covers C, C++, interpreter, Cranelift, WASM, AssemblyScript,
   Rust, Julia, and `-vec -lv 0` / `-vec -lv 1`. Nothing less counts.
3. **The cost is measured, not the shape.** On Cranelift, an `if`-shaped
   block against a counted loop of one iteration around a two-pole filter
   and a `tanh` (10 s of audio, three runs): 3.65, 3.60, 2.71 ms against
   3.70, 3.72, 3.13 ms, at most 15 % and within the spread in two runs of
   three. The peepholes (`if` when the range is within [0, 1] and the flag
   is constant, `for` when the flag is constant) are therefore an
   optimization to decide per backend from such measurements, the
   interpreter first, not a requirement.

**"The same semantics" meaning the same samples, to the bit.** That is
the criterion, and the pins above are not all at that level. Bit-level
today: the `faustprobe` output snapshots (Cranelift, the numbers printed
as the shortest text that reads back to the same double, byte-exact on the
platform of record), and `faustprobe --ref before.npy` or `--compare` at
tolerance 0 on any program. Tolerance-level: the impulse tests
(`filesCompare` at 2e-6 with the bounded overrides of `known.mk`) and the
runtime traces (absolute and relative tolerances): they compare with the
C++ reference, not the compiler with itself, and a one-bit change passes
them. So the before-and-after check for this change is the compiler
against itself, at tolerance 0, per backend: on Cranelift, `--out
before.npy` on the 24 clocked corpus fixtures and the programs of the
clocked tests before the change, `--ref before.npy` after; on the other
backends, the impulse runner's output directories kept before the change
and diffed after (their `.ir` text is the resolution of that diff; where it
prints fewer digits than a double holds, the raw output has to be dumped).

Why the bits can move at all when only a loop header changes: a bounded
`while` against a `for` changes no arithmetic expression; but the FIR
passes that see the loop body (the CSE materializes shared values as
statements) may split an expression at a different place, and C and C++
compilers on arm64 contract `a * b + c` into a fused multiply-add within a
statement, not across two. A different statement boundary is a different
rounding. Cranelift does not contract. That is why the bit-level check is
per backend, and why "no expression changed" is not a proof.

What the change gives up is not semantic: the text parity with the C++
reference's emission (the sample parity stays), and the readability of an
`if (gate)` against a `while` with a flag. What it gives: one guard shape
in the scalar lowering, no `DsModulo` and no per-domain `fDSCounter`, and
the same in the vector lowering once written.

## 3. Surface

The fourth kind is added to every arm that names the three others. From the
sites that name `Upsampling` today:

| layer | files, arms |
|---|---|
| parser | `grammar/faustlexer.l` (keyword), `faustparser.y` (rule), node builder |
| boxes | `builder.rs`, `matcher.rs` (2), `print.rs` (3), tests |
| eval | `apply.rs` (arity: `u+1 → v`) |
| propagate | `flat.rs` (6), `engine.rs` (7), `arity.rs`, `ui_build.rs`, `profile.rs`, `error.rs`, `result_memo.rs`, `clock_domain.rs` (2), `forward_ad.rs` (6), `reverse_ad.rs`, `stateful_rad.rs`, tests |
| signals | `lib.rs` (tag, `SigMatch::Repeat`, builder, dump), tests |
| sigtype | `rules.rs` |
| normalize | `normalform.rs` |
| transform | `clk_env/mod.rs`, `signal_prepare/verify.rs`, `hgraph/mod.rs` (2), `signal_fir/module/clocked.rs` (4, plus the new shape), `core_lowering.rs` (3), `delay/plan.rs`, `tests/coverage.rs`; vector: `clock_ad/{build,check,model,simulation,tests}.rs`, `assemble/{check,materialize}.rs`, `analysis/{effects,dependencies}.rs`, `lower/signal.rs` |
| codegen | interpreter `WhileLoop` (D3); the other backends unchanged; the vector lowering of the new kind (`vector/lower/signal.rs`, the `vclock` loop with an exit) and its `clock_ad` model are new code, since vector mode lowers clocked blocks (§2.6) |
| draw, box-ffi | `draw/translate.rs` (2), `schemas/multirate.rs`, `box-ffi/src/lib.rs` |
| docs | `docs/ondemand-note-{en,fr}.md`, `docs/README.md` (the primitives list), `docs/diagnostics-codes-reference-en.md`, `docs/faust-error-model-en.md`, `libraries/optimizers-overview-{en,fr}.md` §2.5, the tutorial |
| library | `optimizers.lib`: `newton_iter` (§4, W4) |

Some forty files, most of them one arm. The structure gate holds
`clocked.rs` under 2000 lines and its functions under 200; the new shape
goes into functions of its own.

## 4. Plan

Order after §1.5: W4 alone lands the semantics (the library `repeat`, the
solver on it, the documents); W1 to W3 are the compiler primitive, kept
here for the code generation, and W5 qualifies whatever lands. Each phase
has its producer, its check written before the producer, and the
hand-applied mutation the check must reject, per the house method.
Gates at the end of every phase: the crate tests, `golden-check`,
`code-graphs --check`, `structure-check`, clippy `-D warnings`, fmt.

### W1, front end and propagation

Producer: keyword, grammar rule, box tag and builder, print and draw, eval
arity, `FlatNodeKind::Repeat`, propagation to `SIGREPEAT` with
`ClockDomainKind::Repeat`, constant-clock folds, sigtype, normal form,
signal_prepare acceptance, and the clean refusal by `signal_fir`
(`FRS-SFIR-0007`, "not lowered yet") until W2, as P0 did for the three
others.

Check: `crates/compiler/tests/ondemand_pipeline.rs` gains the arity rule
(`(_, _) : repeat(\(x).(x + 1, x > 3))` has 2 inputs, 1 output; a body
with the flag alone is refused), the DAG shape through
`--dump-sig-dag` (the last lane a `Clocked` without `PermVar`, the others
`PermVar`), the folds (`H` constant 0 and 1), the memo (600 references of
one shared `repeat` box share domains, as `clocked_shared_box_one_domain`),
and the structured refusal by `signal_fir`.

Mutation: the flag wrapped in a `PermVar` like a hold; the shape test
fails.

### W2, lowering and backends

Producer: §2.3 in `clocked.rs` (a function per new piece: the shape, the
flag store, the `WhileLoop` assembly), the interpreter's `WhileLoop` (D3),
FIR verifier fixtures.

Check, structural (`clocked_emission_structure.rs`): the C++ of the solver
holds one `while ((lOd0 < ...) & (iStop0 == 0))` and no `for` for the
`repeat` block; `iStop0` is assigned after the `fPerm` stores and before
the cursor bump. Check, numeric (interpreter through
`run_interp_with_inputs`, Cranelift through `faustprobe` in the output
snapshots): flag never raised, equal sample for sample to the integer
`ondemand` with the same body; flag raised at iteration `k`, a `~` counter
in the body reads `k + 1`; flag raised at iteration 0, exactly one; `H =
0`, the outputs hold; local time, a delay in the body counts executed
iterations only; an `repeat` nested in an `ondemand` and in an `repeat`.
The overview's solver written with `repeat` against the unrolled
`newton(8)`: equal to 1e-12, and the steps counted equal those of the
two-layer form on the four inputs of §1.2.

Mutations: `iStop` stored before the hold stores (the "flag raised at `k`"
count is off by one); the `lOd` increment dropped from the sample-end phase
(the never-raised case runs forever: the test has a budget and fails on the
count); the interpreter's condition evaluated after the body (the `H = 1`
with a false flag runs twice).

### W3, differentiation

Producer: `augment_block` for `Repeat`, primal only for the flag lane;
`rad` messages.

Check: the tangent of the solver's solution with respect to `fb` on a
constant input equals the central difference (−0.240418 against −0.24042
in the two-layer form); a structural test on the augmented payload's arity,
`1 + v (1 + n) + 1` lanes for `n` seeds; `rad` across an `repeat` refused
with the kind named.

Mutation: the flag lane augmented like a hold; the arity test fails, and
the lowering refuses the block.

### W4, library and documents (first)

Producer: `op.repeat(C)` as in §1.5, `op.newton_iter(K, tol, F)` on it (the
solver of §1.2 in one layer: warm start, the outer skip on the held
solution, the exit on the residual, its step count as a second output so
that a program can see a budget hit), both with their `#### Test` block;
`libraries/optimizers-overview-*.md` §2.5 rewritten around them, keeping
the two-layer form as what they are made of; `docs/ondemand-note-*.md` with
the pattern (a bounded loop with an exit, in the "recipes" of the note).

Check: `crates/compiler/tests/optimizers_lib.rs` runs the four semantics
cases of §1.5 on `op.repeat` (5, 1, 3 then 2, hold) and `newton_iter`
against `newton(8)` on the four inputs, steps counted; the tangent of the
solution through it against the central difference; the doc numbers
re-measured with the library form and written from the measurement.

Mutation: the `first` test removed from the gate (a flag that fell keeps
the block from ever firing again: the "flag always 0" case counts 1 in all,
not 1 per sample).

### W5, qualification

Corpus fixtures `tests/corpus/repeat_*.dsp` (golden-eligible: no
project-local library import), `golden-check` blessed and its diff read;
the output snapshots re-recorded for the new `faustprobe` cases; D4
settled; `code-graphs`, `structure-check`, `emission-determinism`; the
journal entry; §8 of this document.

## 5. The checks written before the change

Before W1: the arity and shape tests of W1, the structural and numeric
tests of W2, the tangent test of W3, all written against the current
compiler, where they fail on the parse error of the unknown keyword; the
two-layer programs of the overview are their references (steps counted,
values, tangent), recorded now so that the primitive is held to them.

## 6. Risks

- **The keyword.** A new keyword breaks any program using it as an
  identifier; `repeat` and `until` have no use in `faustlibraries` and the
  project libraries today (D1), `loop` has 24.
- **The `WhileLoop` emitters** have never run on a real program: their
  first fixtures are this feature's; the FIR verifier and the structural
  tests are the net.
- **The interpreter** is the one backend with real work (D3).
- **The flag's evaluation point** is the one semantic subtlety: after the
  stores, before the cursor bump; the mutation of W2 guards it.
- **Non-termination** is excluded by construction, `H` bounds; but a
  budget too small is silent, as in §1.2. `newton_iter` should expose its
  step count (or a "hit the budget" flag) so that a program can see it.

## 7. Alternatives considered

- **No compiler change, the library `repeat` of §1.5**: the semantics in
  full, the exit read on the iterate, one layer for the user; the one taken.
- **No compiler change, estimate the count** from the residual of the warm
  start by the quadratic convergence: keeps everything, trims part of the 4
  to 12 %; the proxy stays a proxy. Superseded by the library form.
- **A `while` without bound**: rejected, §1.3.
- **A `break` signal primitive** inside an integer `ondemand`, a
  side-effecting node like `attach`: its evaluation point in the iteration
  would be implicit; the wrapper form makes it a rule.
- **A convention on the last output of `ondemand`** instead of a new
  keyword: changes the meaning of existing programs silently; rejected.

## 8. Status

2026-09-18. §1.5 measured the same day: the library `repeat` reproduces
the semantics with the existing wrappers, and the `fad` defect it uncovered
is fixed (`forward_ad.rs`, `transform_seq`: a literal tangent is not
sequenced through the block; the nested-flag shape is its test, the
reverting mutation fails it). Gradient of the solver through the library
`repeat` on a constant input: −0.240418 against −0.24042 by central
difference, as through the two-layer form. W4 (the library
function, `newton_iter`, the documents) not started; W1 to W3, the
compiler primitive, deferred, D1 to D4 open (§2.8).

2026-09-19. The primitive renamed `repeat` (D1, the name; the polarity
stays open). Its semantics, the three wrappers as its derived forms and
the uses that motivate it are written up without the implementation in
`docs/repeat-note-en.md` and `docs/repeat-note-fr.md`.
