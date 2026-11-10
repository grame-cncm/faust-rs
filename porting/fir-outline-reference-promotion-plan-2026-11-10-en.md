# `fir::outline`: references promoted to struct fields — plan (2026-11-10)

Status: implemented 2026-11-10 (outcome §6).

Parent: the study `porting/cpp-isomorphic-outlined-functions-study-2026-11-10-en.md`,
option B. User decision: in FIR (a FIR-to-FIR step of `fir::outline`), so
only `--outline` output changes.

## 1. Problem

After outlining, a local that a block stores into stays declared in its
method and is passed to the block's function by address. In the motivating
program (36 × 36 gated delay lines), `compute` keeps 23,328 such locals
(`fSlowOD*`, the block-rate values of each `ondemand`, and their
`iSlowODDone*` flags), live across the sample loop: clang spends most of
the build on `compute` (66 s), and the code runs slower than with fields.
The study measured the rewrite by hand: 31 s and 28 % faster at run time,
outputs bit-identical.

## 2. Deliverables and pass criteria

1. `OutlinePolicy::promote_references` (off by default, off for Mojo, on
   for the C++ CLI) and `OutlineReport::promoted`.
2. A producer (`outline/promote.rs`) and an independent checker
   (`outline/promote_check.rs`), with rejecting mutations.
3. Pass criteria: unit tests (promotion, refusals, mutations); the C++
   corpus and numeric tests with promotion on (bit-identical outputs); the
   motivating program measured; Mojo and plain C++ output unchanged.

## 3. The promotion

Runs after outlining and its checker, on the outlined module.

A local `x` of a method `M` (a function the pass did not create) is
promoted when:

- `x` is declared at the top level of `M`'s body, `DeclareVar(x, T, Stack,
  Some(init))` with `T` a scalar;
- `x` is passed by reference to at least one function outlined from `M`;
- every other occurrence of `x` in `M` is an argument `LoadVarAddress(x)` of
  a call to an outlined function;
- `x` is declared nowhere else in the module.

Rewrite:

- the struct gains `DeclareVar(x, T, Struct, None)`;
- in `M`, the declaration becomes `StoreVar(x, Struct, init)` at the same
  place, and the arguments `LoadVarAddress(x)` are dropped;
- in an outlined function `g` that received `x`, the parameter is dropped;
  `LoadTable(x, FunArgs, 0)` becomes `LoadVar(x, Struct)`,
  `StoreTable(x, FunArgs, 0, v)` becomes `StoreVar(x, Struct, v)`, and a
  `LoadVar(x, FunArgs)` passed on to a nested outlined function `h` is
  dropped, `h` treated the same way.

The producer gives up (returns the module unchanged) on any shape outside
these, rather than guess.

**Exactness.** A DSP method is not re-entrant on one instance, and `x` is
named nowhere else, so the field is accessed only by `M` and the functions it
calls with `x`, during one call of `M`. The store at the declaration point
gives the field the value the local had, before any access (the declaration
dominates its uses); every access through the reference is an access to the
same cell. The field's value between two calls of `M` is never read.

## 4. The checker

`check_promotion(src, src_module, dst, dst_module)` shares nothing with the
producer beyond the FIR matcher and builder. Rules:

1. **sections.** Header, globals, static declarations and sub-modules
   identical by id; the struct is the source struct followed by new
   `DeclareVar(x, T, Struct, None)`, `T` scalar, `x` distinct, neither a
   function nor declared in a section of the source (it is a local of a
   body, which rule 4 checks): the promoted set `P`.
2. **signatures.** Same functions in the same order, same name, return type
   and inline flag; each function's arguments are the source's minus a set
   `R_f ⊆ P`, each removed one named `x` and typed `Ptr(T_x)`.
3. **declarations.** No function declares a name of `P`.
4. **demotion.** Rebuild the source from the output: in a function with
   `R_f` empty, the first top-level `StoreVar(x, Struct, v)` of each `x ∈ P`
   becomes `DeclareVar(x, T_x, Stack, Some(v))` and its other `Struct`
   accesses to `x` become `Stack`; in a function with `x ∈ R_f`,
   `LoadVar(x, Struct)` becomes `LoadTable(x, FunArgs, 0)` and
   `StoreVar(x, Struct, v)` becomes `StoreTable(x, FunArgs, 0, v)`; every
   call to a function `g` with `R_g` non-empty gets its arguments back at the
   positions of `g`'s source signature, `LoadVar(x, FunArgs)` when the
   caller itself received `x`, `LoadVarAddress(x, Stack)` otherwise. The
   result must be the source module by id.

**Mutations.**

| mutation | defect | rule |
|---|---|---|
| `PromoteKeepDeclaration` | the local declaration kept, the field never set | declarations |
| `PromoteDropReset` | the declaration dropped, no store | demotion |
| `PromoteSwapFields` | two promoted fields swapped in one function | demotion |

## 5. Wiring

`outline_large_blocks` runs the promotion when the policy asks for it and
something was outlined, and returns its output only once the promotion
checker accepts it (fail closed). The report lists the promoted names and
drops them from each function's `by_reference`. The C++ CLI policy turns it
on; the C++ emitter needs nothing new (a struct scalar is a member).

## 6. Outcome (2026-11-10)

The motivating program, N = 36, Apple clang `-O3 -ffast-math`, runtime per
256-frame block at 48 kHz:

| | faust-rs | clang | all gates | 1 gate in 36 |
|---|---|---|---|---|
| `--outline` before | 10.1 s | 66 s | 8.7 ms | 0.205 ms |
| `--outline` with promotion | 11.1 s | 34 s | 6.4 ms | 0.154 ms |

23,328 locals promoted; outputs bit-identical to `--outline` before. The
promotion and its check cost 1.2 s (§ `porting/MEMOIZATION.md` 2.32 for the
two quadratic first versions). Tests: 7 new unit tests in
`crates/fir/src/outline/tests.rs`; `crates/compiler/tests/cpp_outline.rs`
runs its corpus and numeric checks with promotion on.
