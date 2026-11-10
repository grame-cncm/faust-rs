# Isomorphic outlined functions: sharing one body — study (2026-11-10)

Status: study. Option B implemented 2026-11-10 in `fir::outline`
(`porting/fir-outline-reference-promotion-plan-2026-11-10-en.md`: clang
34 s, 6.4 ms / 0.154 ms, as the prototype); A rejected; C not started. Follows `--outline` for C++
(`porting/cpp-outlining-plan-2026-11-10-en.md`, commit `0210fee4`).

## 1. Question

With `--outline`, the motivating program (a 36 × 36 matrix of gated delay
lines, each `(gate, _) : ondemand(delay : gain : fi.highpass(8))`) gives
1,298 methods `computeOutline<k>`. Is their code the same each time, and can
one body serve them all?

## 2. Finding

Textually all 1,298 differ. With every name stripped of its number they
reduce to three shapes:

| methods | lines | what |
|---|---|---|
| 1,296 | 125 | one line of the matrix (its `ondemand` block), identical up to renaming |
| 1 | 1,994 | phase-2 run: the `iSlowN = (int)fCheckboxN` gate reads, first part |
| 1 | 598 | the same, second part, through the spill array `computeSpill0[1296]` |

What differs between two line methods is only names:

- 53 struct fields per line, none shared with another line: 4 sliders, 3
  constants, 24 two-element recursion arrays `fRec*_d0`, a scalar recursion,
  the domain counter `fIOTA_d*`, the delay buffer `fVec*_d*[131072]`, the
  output `fPerm*`, and, once promoted (§4, B), the 18 block-rate values
  `fSlowOD*`/`iSlowODDone*`;
- the parameters: 17 `fSlowOD*` and one `iSlowODDone*` by reference, `i0`,
  and the input channel `input<row>`;
- the locals (`fTemp*`, `fRecBody*`, `fRecCur*`), numbered program-wide.

The parameters are also ordered differently: the pass sorts them by name,
so `fSlowOD9` comes before `fSlowOD10` in one line and after it in
another. Ordered by first use instead, all 1,296 bodies match.

**Hash-consing.** `FirStore` interns a node by its kind and children, and a
variable's name is a child, so the 1,296 bodies are 1,296 different ids
today, and so is every statement that names a variable. Once a body is put in
canonical form — fields accessed through the instance (§4), locals renamed
by order of first declaration, parameters ordered and named by first use —
the 1,296 canonical bodies are one tree, and building them in one store
gives the same `FirId` automatically. Detecting isomorphic functions then
costs one id comparison per function, no structural matching.

## 3. Where the C++ compiler spends its time

Profiled with `clang -ftime-trace` (Apple clang, `-O3 -ffast-math`). In the
`--outline` output, the 1,296 line methods each compile in linear time; what
remains expensive is `compute` itself: 26 s of optimization plus most of the
35 s of code generation. It still holds 22,032 `float fSlowOD*` and 1,296
`int iSlowODDone*` locals, live across the whole sample loop and each
passed by address to its line's method, then a loop body of 1,296
`if (gate[k]) computeOutlineK(&..., i0, input)`.

## 4. Prototypes

Text rewrites of the N = 36 `--outline` output, each checked by building it
and running it (scripts kept out of the repository; the transformations are
described exactly here):

- **A — one shared function, fields passed by pointer.** Each line method
  becomes a call to a static `computeShared0` taking a pointer to each of its
  fields (53 + its 21 parameters, about 56 arguments).
- **B — block-rate locals as fields.** The 23,328 `fSlowOD*` and
  `iSlowODDone*` locals of `compute` become class fields, reset where the
  locals were declared (the head of `compute`), so the semantics are
  unchanged; the line methods take only `(i0, input)`.
- **C — an array of line instances and a loop.** On top of B, the 53 fields
  of a line become a `struct Line`, the class holds `Line fLines[1296]` and
  a table `fLineInput[1296]` of input indices, one method
  `lineCompute(Line*, input, i0)` replaces the 1,296 methods, and the 1,296
  `if`s of the sample loop become
  `for (k) if (computeSpill0[k] != 0) lineCompute(&fLines[k], inputs[fLineInput[k]], i0)`.
  Every other reference to a line's field (initialization, clear, UI, the
  output sums) becomes `fLines[k].Fj`. "C rerolled" also folds the 23
  per-line resets at the head of `compute` into 23 loops over `k`.

## 5. Measurements

Same machine, one run each. Runtime: 100 blocks of 256 frames at 48 kHz
(real-time budget 5.33 ms per block), all 1,296 gates on, or one gate in 36.

| variant | clang | all gates | 1 gate in 36 | output |
|---|---|---|---|---|
| plain (without `--outline`) | 1,690 s | 9.0 ms | 0.233 ms | reference |
| `--outline` (`0210fee4`) | 66 s | 8.7 ms | 0.205 ms | = reference up to `-ffast-math` |
| A, wrappers inlined by clang | 117 s | — | — | — |
| A, wrappers kept out of line | 57 s | — | — | — |
| B | 31 s | 6.3 ms | 0.150 ms | = `--outline`, bit for bit |
| B + A | 27 s | 17.7 ms | 0.364 ms | = `--outline`, bit for bit |
| C | 71 s | 5.4 ms | 0.300 ms | = `--outline`, bit for bit |
| C rerolled | 21 s | 5.4 ms | 0.305 ms | = `--outline`, bit for bit |

"Up to `-ffast-math`": the plain build differs from the others by at most
7.5 × 10⁻⁹ for an output RMS of 3 × 10⁻³ (reassociation of different code);
all the transformed builds are bit-identical to each other.

Readings:

- **A is rejected.** Sharing through pointer arguments makes the program
  2.8 times slower than B (each call sets up 56 arguments, most on the
  stack, and the body loses constant offsets from `this`), and clang
  inlines the 1,296 wrappers back into `compute` unless told not to (117 s).
  Sharing the body was not where the compile time was.
- **B is the cheap, large win.** Half the compile time of `--outline`, and
  28 % faster at run time: locals passed by address cost both.
- **C is the structural fix.** `compute` drops to 119 lines and one line
  body is compiled once; the build time left (21 s) is the initialization,
  reset and UI functions, still unrolled per line. It is the fastest with
  dense gates (5.4 ms, under the budget only just) but slower with sparse
  gates (0.30 against 0.15 ms): a loop over 1,296 gates per sample with an
  indirect input load, instead of straight-line tests on constants.
  Unrolled C (one body, 1,296 calls `lineCompute(&fLines[17], input3, i0)`)
  would keep B's sparse speed; not measured.
- C unrolled but with its 23,000 resets at the head of `compute` costs 71 s,
  48 s of it in `MemCpyOptPass` trying to merge stores into one large
  object: a C design must also reroll what touches the instances outside
  the sample loop, not only the loop.

## 6. Design options for faust-rs

**B in FIR (recommended first).** The `fSlowOD*` values are the block-rate
computations of a boolean `ondemand` run at its first fire (DIFF-BEH-024),
held in locals of `compute` with a done flag. Two places can make them
fields:

- in the ondemand lowering (`transform`): declare them in the struct and
  reset the done flags at the head of `compute`; the cleanest, it also
  removes them from the scalar output without `--outline`;
- in `fir::outline`: promote a caller local that is declared with a
  constant initializer at the top of a method body and accessed only inside
  one outlined block. Exactness argument: `compute` is not re-entrant on one
  instance, so a field re-initialized at the local's declaration point holds
  the same value at every access. Checker: inline-back with the field turned
  back into the local.

Extra memory: 23,328 scalars, 93 KB per instance, against 680 MB of delay
lines.

**C in FIR (the structural fix).** A pass after outlining:

1. canonical form of each outlined function (above) and grouping by
   `FirId`;
2. for a group of more than one function whose fields are disjoint between
   members: struct-of-arrays layout — each canonical field `Fj` becomes a
   struct array `Fj[n]` (or `Fj[n][dim]`) indexed by the instance, so no new
   FIR type is needed; the shared function takes the instance index `k`;
3. the calls: kept unrolled with a constant `k` (no runtime cost), or
   rerolled into a loop when they are consecutive and their arguments are
   affine in `k` or tabulated (`fLineInput`);
4. initialization, clear, UI reset and the output reads rewritten to
   `Fj[k]`, rerolled into loops where they are regular (the UI cannot be
   fully rerolled: labels differ, `"d%n"`, unless they are tabulated).

This changes the class layout (fields grouped into arrays), but not the
`dsp` contract: UI zones remain addresses of floats (`&fHslider[k]`), and
`-uim` macro field names would change. It is a finite structural artifact
consumed downstream whose errors would be silent, so it falls under the
producer/checker methodology (AGENTS.md §5): the checker un-rerolls and
un-shares — every call replaced by the shared body with `k` substituted,
every `Fj[k]` renamed back to the member's field — and must get the
`--outline` module back by id.

**Not recommended:** A (§5).

## 7. Open questions for the user

1. B: in the ondemand lowering (also changes the default output, a codegen
   golden refresh) or in `fir::outline` (only under `--outline`)?
2. C: worth a plan of its own? It changes the class layout and the `-uim`
   field names; with dense gates the program is just over real time on this
   machine anyway (5.4 ms for a 5.33 ms budget).
3. Unrolled or rerolled calls in C: unrolled keeps sparse gates fast,
   rerolled shrinks `compute` to a loop.
