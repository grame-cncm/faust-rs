# C++: large programs in useful compile time — plan (2026-11-10)

Status: implemented 2026-11-10 (outcome §6).

Parent: the outlining pass written for the Mojo backend
(`porting/mojo-large-program-outlining-plan-2026-11-09-en.md`, `fir::outline`).
Motivation: a user program, a 36 × 36 matrix of gated delay lines
(`par(i, 36, _ <: par(j, 36, line(i, j)))`, each line
`(gate, _) : ondemand(delay : gain : fi.highpass(8))`), whose generated C++
takes minutes to build.

## 1. Problem

faust-rs writes the program's C++ in about 9 s; the C++ compiler then spends
most of the build on one function. All 1,296 lines of the matrix fall in the
one sample loop of `compute`, about 135 statements each inside the `if` of its
gate: 190,000 lines in one function. Measured with Apple clang, `-O3
-ffast-math`, on reduced matrices of the same program:

| N | lines of C++ | clang |
|---|---|---|
| 6 | 10,000 | 1.0 s |
| 12 | 40,000 | 10.5 s |
| 18 | 89,000 | 54 s |

The time grows as the square of the function's size. `-O1` does not help
(8.6 s at N = 12) and `-vec` falls back to scalar on this program. Making the
program smaller helps only by a constant (`fi.highpass(2)`: 4.4 s at N = 12).

## 2. Deliverables and pass criteria

1. `CppOptions::outline: Option<OutlinePolicy>`, off by default, and the CLI
   option `--outline` (C++ only) that sets it to the default policy with
   pointers passed by value (§3).
2. The C++ emission of the outlined functions (§4).
3. Two widenings of `fir::outline`, with checker rules and rejecting
   mutations (§3).
4. Pass criteria:
   - without `--outline` the C++ output is unchanged (codegen-identity
     differential over every `.dsp` of `tests/` and `examples/`);
   - a program under the budget is emitted unchanged with `--outline`;
   - every hermetic program of `tests/corpus` and `tests/impulse-tests/dsp`
     emits C++ under a policy that outlines every block, and one that also
     cuts runs, whenever it emits C++ at all;
   - the programs those policies change compute bit-identical outputs to
     their plain build (built with the in-repository minimal architecture);
   - the motivating program builds with clang in a time near linear in N;
   - the Mojo output is unchanged (its policy leaves the widenings off).

## 3. Widenings of the pass

**Sub-modules kept.** The pass refused a module with sub-modules (Mojo
flattens the generated-table classes first; C and C++ emit them as classes).
It now rewrites the root's functions only and rebuilds the module with its
sub-modules unchanged; checker rule 1 already compares them by id.
`OutlineError::SubModules` is removed.

**Pointers by value (`OutlinePolicy::pointer_params`).** Every `ondemand`
block of the motivating program reads its input channel, `input0[i0]`, where
`input0` is a local `FAUSTFLOAT*` of `compute`; the pass refused any free
pointer, because Mojo cannot name the origin of a pointer parameter. With
the flag, a free local of type `Ptr` of a scalar that the block never stores
into, takes the address of, tees or shifts is passed by value: a parameter of
the same `Ptr` type, the call passes `LoadVar(p)`, and inside, its loads and
its table accesses at any index go through `FunArgs`. This is C semantics
exactly: the pointer is copied, the pointee shared. Off by default; Mojo
leaves it off and its output is unchanged.

Refusal reasons: `a store, address, tee or shift of a free pointer`. The
analysis now tells a table access at a non-zero index (`indexed`) apart from
an address, tee or shift (`other`), so the earlier refusals keep their
reasons.

**Checker.** Rule 2 cannot tell a pointer by value from a reference: both are
`Ptr(T)`. Rule 3 does, by the argument: a `Ptr(T)` parameter whose argument
is `LoadVar` of a caller's variable that is not one of the caller's
references is `Passing::Pointer` (callers first, so a pointer passed on
keeps its kind). Rule 4: a pointer parameter is reached only by a load or a
table access at `FunArgs`, never stored, addressed, tee'd or shifted. Rule 6
(inline-back): its loads and table accesses at `FunArgs` become accesses at
the argument's access type, indices kept.

**Mutations.**

| mutation | defect | rule |
|---|---|---|
| `PointerAsReference` | the pointer passed by address | closed (a reference reached at `i0`) |
| `PointerStore` | the pointer parameter stored into | closed |

## 4. C++ emission

- `outline_module` runs the pass first, when the option asks for it, and
  hands the emitter the new module and, from the pass's report, the
  parameters each function receives by reference.
- An added function (first argument `dsp: Ptr(Obj)`, neither a lifecycle
  nor a sub-module method) is a method of the class, emitted after the
  lifecycle methods, without the `dsp` argument (struct state through
  `this`). A reference parameter is `T* RESTRICT name`: the addresses of
  distinct locals alias neither each other nor the struct. A pointer by
  value is `T* name` without `RESTRICT`, since a host may pass the same
  buffer as input and output. A spill array is an array parameter.
- The call statement `Drop(FunCall(f, [LoadVar(dsp), ...]))` prints
  `f(args)`, `LoadVarAddress(x)` as `&x`.
- The methods are member functions defined in the class, so `inline` by
  linkage; clang keeps them separate at `-O3` (each is called once but is far
  above its inlining threshold): `nm` lists the 144 methods at N = 12.

## 5. Qualification

1. `crates/fir/src/outline/tests.rs`: a pointer passed by value (read, and
   written at `i0`), refused without the flag, refused when stored into,
   both mutations rejected, the unmutated producer accepted.
2. `crates/compiler/tests/cpp_outline.rs`: shape (methods on `this`,
   `RESTRICT` only on addresses, unchanged under the budget), coverage of the
   two corpora, numeric bit-identity of plain, every-block and every-run
   builds (three classes in one translation unit, skipped without a C++
   compiler).
3. Codegen identity without `--outline`; the Mojo corpus test unchanged.
4. The AGENTS.md gates; `code-graphs` regenerated (`OutlinePolicy` gained a
   field, `OutlineError` lost a variant, `CppOptions` gained a field).

## 6. Outcome (2026-11-10)

The motivating program, N = 36, Apple clang `-O3 -ffast-math`:

| | faust-rs | clang |
|---|---|---|
| plain | 8.9 s | 1,690 s (28 min; wall clock, the machine partly busy with other builds) |
| `--outline` | 10.1 s | 66 s |

and on the reduced matrices: N = 12, 10.5 s → 7.0 s; N = 18, 54 s → 9.0 s.
The pass costs faust-rs about 1.2 s on this program (1,298 functions
outlined).

The 48 hermetic impulse programs the test policies change compute
bit-identical outputs to their plain build; 288 corpus and impulse programs
emit C++ under both policies, 223 of them changed.

Not done: the C backend (same FIR, its own method convention with an
explicit `dsp` pointer); a CLI knob for the budget.
