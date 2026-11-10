# Mojo: large programs in useful compile time — plan (2026-11-09)

Status: phase 1 implemented 2026-11-09 (outcome §9); phase 2 (§3.4) planned
the same day, after phase 1's measurements, outcome §10.

Parent: the batch contract of the Mojo backend
(`porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`) and its
state promotion (`porting/batch-state-register-promotion-plan-2026-11-08-en.md`).
Motivation: phase P0 of the recorded-piano fit of the sibling project
`faust-coopiano` (`docs/recorded-piano-fit-plan-2026-11-08-en.md`, report
`results/fit/p0.txt`).

## 1. Problem

Mojo 1.3 nightly (`26.8.0.dev2026110705`) compiles faust-rs output for very
large programs too slowly to be usable:

1. **One large function.** Mojo lowers a function in time much worse than
   linear in its size: 8,000 statements take 19 s, 16,000 take 491 s
   (synthetic, P0). The coopiano voice's `compute` (56,000 lines, 51
   `ondemand` blocks inside the sample loop) did not build in 3.5 h.
2. **Large table literals.** A 12,001-entry `F32` array literal in
   `__init__` takes 57 s alone (measured 2026-11-09: 256 entries 1.2 s,
   512 entries 1.3 s, 2,000 entries 3.5 s, 12,001 entries 57.5 s, each a
   whole program). The voice has two of them.
3. **Many locals carried across the sample loop.** With the batch state
   promoted to locals (`fir::batch::promote_state`), the voice's `compute`
   carries about 2,300 state elements across its sample loop; P0 measured
   2,000 such locals at 446 s. The voice is therefore built with
   `mojo_batch --no-registers`, and runs on the GPU 2.2 times slower than on
   ten CPU threads.

`faust-coopiano` works around 1 and 2 with two text rewrites of the
generated Mojo (`fit/mojo_outline.py`, `fit/mojo_tables.py`): every
top-level `if` of the sample loop moved into its own method, and the large
literals moved to binary files loaded at init. With both, the plain voice
builds in 90 s and the one-tangent voice (`fit/voice_dir.dsp`) in 145 s.

Since `0d7e4a9b` (one generated table per content, `DIFF-BEH-029`) the voice
has 4 table fields instead of 121 literals (2,200, 264, 12,001 and 12,001
entries); problem 2 remains for the two large ones.

## 2. Deliverables and pass criteria

1. A FIR-to-FIR pass, `fir::outline`, that moves large blocks of a function
   into new functions of the module, with an independent checker and
   rejecting mutations (§3, §4).
2. Mojo emission of the outlined functions as struct methods, and of large
   struct tables as packed text decoded at init (§5, §6).
3. Both on by default in the Mojo backend, batch and scalar, through
   `MojoOptions`; a program under the budget is emitted exactly as before.
4. Pass criteria:
   - the coopiano voice (`fit/voice.dsp`) and one-tangent voice
     (`fit/voice_dir.dsp`), generated as its Makefile does
     (`mojo_batch --no-registers`), build with Mojo **without the two
     scripts** in time comparable to 90 s and 145 s;
   - their GPU output equals the scripted build's, bit for bit or within f32
     rounding;
   - a corpus test runs the pass over every program of `tests/corpus` with a
     budget small enough to outline every candidate, and every output is
     accepted by the checker and by the FIR verifier and emits as Mojo;
   - Mojo output of every program under the budget is unchanged
     (codegen-identity differential over every `.dsp` of `tests/` and
     `examples/`, scalar and batch), except for packed tables (§6).

## 3. The pass

`fir::outline::outline_large_blocks(src, module, policy)` rewrites a copy
of a flattened module (no sub-modules; since 2026-11-10 a module's
sub-modules are kept as they are, for the C++ backend,
`porting/cpp-outlining-plan-2026-11-10-en.md`). It runs last, on the module the
emitter will print: in the batch path after `promote_state` and
`layout_voices`, so neither of them nor their checkers ever sees an outlined
function.

### 3.1 Policy

```text
OutlinePolicy { budget: 2_000, min_block: 64, split_budget: 8_000 }
```

Size is the number of FIR statements of a subtree, nested ones included (a
call counts one). It is the proxy P0 measured Mojo's cost with. The pass
targets the DSP methods with a body: `compute`, `instanceConstants`,
`instanceResetUserInterface`, `instanceClear` and `staticInit`, then the
functions it creates.

For each target function `F`, while `size(F) > budget`:

- the **candidates** are the `then` and `else` blocks of the `If`
  statements of `F`, at any depth, with
  `min_block <= size(B) <= size(F) - min_block` (the second bound keeps an
  outlining from leaving a function that is only a call);
- a candidate is refused if it cannot be outlined exactly (§3.3);
- the largest candidate is outlined, ties broken by pre-order position;
- if there is none, `F` stays as it is.

Each new function is then processed the same way. A program whose methods
are all under the budget comes out unchanged (the same root id).

The `If` itself stays in the caller: an `ondemand` block's function is
called only on the samples where its clock fires. `faust-coopiano`'s script
moved the whole `if` and paid one call per block per sample.

The budget (2,000 statements) is below the size where Mojo's cost becomes
large (8,000 statements, 19 s) and above the voice's sample loop without its
blocks (2,581 lines), whose blocks it outlines all. `min_block` keeps small
conditionals inline.

### 3.2 Parameters

Inside a block, a variable is **free** when it is accessed as `Stack`,
`Loop` or `FunArgs` without being declared in the block. `Struct`, `Static`
and `Global` accesses stay as they are: the new function is a method, with
`dsp: Ptr(Obj)` as its first argument, like the lifecycle functions.

- A free variable the block only reads is passed **by value**: a parameter
  of the variable's type, with the variable's name; inside, its accesses
  become `FunArgs`.
- A free variable the block stores into is passed **by reference**: a
  parameter `Ptr(T)` with the variable's name, the call passes
  `LoadVarAddress`, and inside every load becomes
  `LoadTable(name, FunArgs, Int32(0))` and every store
  `StoreTable(name, FunArgs, Int32(0), v)`. This is exact C semantics, so
  any backend with pointers can print it; the Mojo emitter prints it as a
  `mut` argument (§5).

Parameters are sorted by name. The call is the statement
`Drop(FunCall(f, [LoadVar(dsp), args...], Void))`, the block's only item.
A block has no escaping declaration: the declarations of a `then` or `else`
block are scoped to it.

Names: `<function>Outline<k>`, `k` counting per module from 0, a name
already used by a function or declaration making the pass skip to the next
`k`. In Mojo, `compute_outline0`.

### 3.3 Refusals

A candidate block is left inline when it:

- contains a `Return` (it would return from the new function only);
- reads a free variable of pointer, array or vector type (unless
  `OutlinePolicy::pointer_params`, added 2026-11-10 for the C++ backend,
  passes a pointer of a scalar by value; Mojo leaves it off. Mojo cannot name
  the origin of a pointer parameter; arrays are not passed);
- takes the address of a free variable, uses one in a `TeeVar`, shifts one,
  or accesses one through `LoadTable`/`StoreTable`;
- declares a name that is also free in it (a parameter would be shadowed);
- refers to `dsp` explicitly.

The report lists each outlined function (name, parent, size, parameters by
value and by reference) and each refused candidate with its reason.

### 3.4 Phase 2: straight-line runs

Phase 1 leaves whole a block that is nearly all of its method. Measured on
2026-11-09: `ondemand_stft_denoiser_1024` keeps a 52,361-statement
straight-line `ondemand` body (Mojo: 428 s), and the 12-seed `fad` voice of
coopiano has four methods out of reach — `instanceClear` (31,465 lines),
`compute` (a 23,708-statement sample loop body) and two `ondemand` bodies
(21,926 and 11,070 statements).

**When.** After phase 1, a function above `split_budget` (default 8,000
statements) is split. The block split is found by descending from the
function's body into the largest nested block of an item holding more than
half of the current block, so the sample loop body of `compute` is split, not
`compute`'s few statements around it. The voices that build today (largest
method 6,229 statements) are below the default and unchanged.

**Runs.** The block's items are cut into consecutive runs of at most
`budget` statements, each run moved into a new function called in its place,
as in phase 1 (free locals read by value, stored by reference). An item the
analysis refuses (a pointer local, a `Return`, ...) is a barrier: it stays in
the caller and ends the current run. A cut is placed, within the second half
of each run's budget, where the fewest values cross it.

**Escaping values: spill arrays.** A local declared at the top level of a run
and read after it (by a later run or a statement left in the caller) is an
escape. Measured on the 12-seed voice, an `ondemand` body computes thousands
of values and stores them all at its end: 1,000 to 10,000 values cross each
cut of the 11,070-statement block, too many to pass as parameters. Instead,
per element type, the caller declares one local array `<method>Spill<k>`
(`Array(T, n)`, `Stack`, no initialiser) before its first call and passes it
to the runs (a third parameter kind, `Array(T, n)`, by reference):

- the defining run keeps the declaration and its uses unchanged and ends
  with one spill store per escape, `StoreTable(spill, FunArgs, k,
  LoadVar(e))`;
- every later read of `e`, in a run or in the caller, becomes
  `LoadTable(spill, FunArgs or Stack, k)`.

Only a **single-assignment** local may escape: declared once in the block,
with an initialiser, of a scalar type, never stored into, tee'd, shifted or
addressed. A cut that a mutable local would cross is not taken. Then the
value in slot `k` is the value of `e` wherever `e` is read, and inlining the
spill back is exact.

**Checker (rules 2 to 5, extended).** Signature: a parameter may also be an
`Array` of a scalar, passed as `LoadVar(p, Stack)` of a fresh name declared
in the calling block. Closed: such a parameter is reached only by
`LoadTable`/`StoreTable` at an `Int32` literal index in range. **Spills (new
rule):** each slot is stored by exactly one spill statement, at the end of
one run, from a local `e` that is single-assignment in the source body; every
read of the slot is in a call after that run's call, or in a caller statement
after it, and never in the defining run. Inline-back: the spill
declarations and the trailing spill stores are dropped, every slot read
becomes `LoadVar(e)`, and the result must be the source body by id.

**Mutations:** `SpillStale` (a spilled local stored into after its spill,
rejected by spills), `SpillWrongSlot` (a read of the slot of another value,
rejected by inline-back), `SpillEarly` (a run reading a slot before the run
that stores it, rejected by spills).

**Mojo.** An `Array` parameter is a `mut` argument (`mut p: Arr[T, n]`), its
accesses `p[k]`; the caller's spill array is declared zero-filled, as any
local array.

**Pass criteria (phase 2):**
- unit tests: a long run split with escapes through spill arrays, a barrier,
  a mutable local that no cut crosses, the checker accepting, the three
  mutations rejected by their rules;
- the corpus test with a policy that also splits every block;
- every program under the split budget unchanged (codegen-identity
  differential over `tests/` and `examples/`), the coopiano voices included;
- `ondemand_stft_denoiser_1024`: Mojo impulse output identical to the base
  build, Mojo build time well below 428 s;
- the 12-seed `fad` voice (`faust-coopiano/fit/voice_fad.dsp`) builds with
  Mojo in useful time (it did not build at all in P0).

## 4. The independent checker

`fir::outline::check::check_outlining(src, src_module, dst, dst_module)`
shares no code with the producer beyond the FIR matcher and builder. Both
modules are imported into one checking store, where identical trees intern
to one id. Rules, in order:

1. **sections.** Module header, structure, globals, static declarations and
   sub-modules are identical by id; the source functions are the first
   functions of the output, in order, with the same name, type and arguments.
2. **signature.** Every added function has a fresh name, returns `Void`,
   takes `dsp: Ptr(Obj)` first and distinct parameter names; each other
   parameter is a scalar (`Int32`, `Int64`, `Float32`, `Float64`,
   `FaustFloat`, `Bool`) or `Ptr` of one (by reference).
3. **calls.** Every added function is called exactly once, by a source
   function or another added function, as a statement
   `Drop(FunCall(f, [LoadVar(dsp, FunArgs), a_1..a_n], Void))`, where `a_i`
   names the `i`-th parameter: `LoadVar(p_i, acc, T_i)` for a value
   parameter, `LoadVarAddress(p_i, acc)` for a reference, with `acc` one of
   `Stack`, `Loop`, `FunArgs`.
4. **closed.** An added function's body declares none of its parameter
   names and has no other free `Stack`, `Loop` or `FunArgs` variable; a value
   parameter is never stored into; a reference parameter is accessed only
   through `LoadTable`/`StoreTable` at the literal index 0.
5. **inline-back.** Replacing, bottom-up, every call by the body of its
   function — `FunArgs` accesses of a parameter turned back into accesses
   of the caller's variable with the access type of the argument, index-0
   table accesses of a reference into `LoadVar`/`StoreVar` — gives every
   source function body exactly (by id).

Rules 3 to 5 together are the semantic argument: the inlined text is the
source; value parameters are copies of variables nothing writes during the
call (a variable is passed once, either way); references are the variables
themselves; the body sees nothing else of the caller.

`outline_large_blocks` returns only an output its checker accepts, and fails
closed otherwise.

**Rejecting mutations.** The producer has a `Mutation` switch (tests only):

| mutation | defect | rule |
|---|---|---|
| `ByValueWrite` | a written variable passed by value | closed |
| `MissingParam` | a free variable left out of the parameters | closed |
| `SwapArgs` | the first two arguments of a call swapped | calls |
| `DropStatement` | the last statement of a block dropped | inline-back |
| `CallTwice` | the call statement duplicated | calls |
| `ShadowParam` | a parameter name redeclared in the body | closed |
| `WrongIndex` | a reference read at index 1 | closed |

## 5. Mojo emission

- A defined, non-lifecycle FIR function whose first argument is
  `dsp: Ptr(Obj)` is emitted as a method of the struct, after `compute`,
  through the same `write_method` as the lifecycle methods (receiver alias
  when the body loops). Its name goes through the field renamer, which
  catches a clash with a field.
- A `Ptr(T)` parameter of such a method, whose every access is at index 0,
  is a `mut name: T` argument; its accesses print as `name`. Any other use
  makes the emission fail (`FRS-CGEN-MOJO-0002`), rather than print a
  pointer.
- `Drop(FunCall(f, [dsp, ...]))` of a method prints `dsp.f(args)`;
  `LoadVarAddress(x)` as the argument of a reference prints `x`.

## 6. Packed tables

A struct table (`DeclareTable` in the struct, global or static section)
with at least 256 entries of `Int32`, `Float32` or `Float64` is
initialised in `__init__` by a generic helper from a string literal holding
the bit pattern of each entry in hexadecimal (8 or 16 digits):

```text
dsp.ftbl0 = faust_unpack32[f32, 12001]("bfe50f5bbfe1b9f2...")
```

The helper (`faust_unpack32`, `faust_unpack64`) is written at the top of the
unit only when used, like `faust_idiv`. The entry is the FIR literal
converted to the element type in Rust (`as f32`), the conversion Mojo applies
to the literal's digits. Measured: the 12,001-entry table builds in 1.2 s
instead of 57.5 s, and its entries are identical. A stack table in a function
body stays a literal: it would be decoded at every call.

The `__init__` of a batch struct runs on the host (the harnesses build the
struct there, then copy it), so the decoding costs nothing on the device.

## 7. State promotion

Outlining runs after `promote_state`. With promotion on, an `ondemand` block
that stores into promoted state receives it by reference, so the voice's
`compute` would still carry its 2,300 promoted elements across the sample
loop (problem 3). This delivery measures that build once and records it; a
promotion that leaves out the fields stored only inside outlined blocks is
the follow-up if the measurement calls for it.

## 8. Qualification

1. Unit tests in `crates/fir/src/outline/tests.rs`: the pass on hand-built
   modules (a sample loop with two `ondemand`-like blocks, a by-value and a
   by-reference variable, a nested block, the minimum-size bound, each
   refusal), the checker accepting each output, each mutation rejected by
   its rule, and a module under the budget returned unchanged.
2. Corpus: `crates/compiler/tests/fir_outline_corpus.rs` runs every
   `tests/corpus/*.dsp` that lowers to FIR through the pass with
   `budget = min_block = 1`: checker accepted, `verify_fir_module` without
   error, Mojo emitted (scalar and batch).
3. Mojo emitter tests: methods, `mut` arguments, the call statement, packed
   tables (helper written once, entries spelled as bit patterns).
4. Codegen identity (§2.4).
5. Numeric, opt-in (Mojo): the impulse lane through `mojo_batch --scalar`
   with `budget = min_block = 1` against the C++ oracle; the coopiano voices
   (§2.4).
6. The AGENTS.md gates, `compile-budget-check` (`fir` and `codegen` are
   touched), `code-graphs` regenerated.

## 9. Outcome (2026-11-09)

Implemented as planned for phase 1 (§3.1 to §3.3, §4 to §6); phase 2 (§3.4)
is not built.

- `crates/fir/src/outline/mod.rs` is the producer, `check.rs` the checker,
  with the rules of §4 in that order.
- `crates/codegen/src/backends/mojo/methods.rs` prints the outlined methods
  and their calls; `helpers.rs` holds the helpers written on use, the table
  decoders among them.
- `MojoOptions::outline` (default `Some(OutlinePolicy::default())`) and
  `MojoOptions::packed_tables` (default `true`), for `-lang mojo` and the
  batch contract alike; `mojo_batch` gains `--no-outline`,
  `--outline-budget`, `--outline-min-block`, `--table-literals` and
  `--scalar`.

Corrections to the plan from the implementation:

- **Selection in one pass.** §3.1 describes a loop that outlines one block
  and looks again. Written that way the pass re-walked `compute` 51 times on
  the coopiano voice (1 s). The candidates are now collected once per method,
  with their sizes, and taken largest first while the method is over budget,
  skipping a candidate nested in a block already taken or one that would
  leave fewer than `min_block` statements; each block taken shrinks the
  method by its size less one. The selection is the loop's (output identical
  on both coopiano voices): a refused block stays refused once its descendant
  is outlined, because it then holds a call to an outlined function, and the
  upper bound moves with the method's size.
- **Pass-through references.** A block of an outlined function that reaches
  one of that function's reference parameters receives it as a reference in
  turn (the call passes the pointer, `LoadVar(p, FunArgs, Ptr(T))`). The
  checker's rule 3 admits that argument form only from an added function
  whose parameter it is, and rule 5 inlines callees first.
- **The `ShadowParam` mutation** first redeclared the parameter with its own
  value as initialiser; rule 4 rejected the load of the reference before it
  reached the declaration. It now declares without initialiser, so it is the
  declaration rule that rejects it. Every mutation is rejected by its rule.
- **A replacement block is not a candidate.** A `then` block reduced to the
  call of its function was offered again (and refused); such blocks are
  skipped.
- **Pointer locals.** A block that reads `input0`/`output0` (a pointer local)
  is refused (§3.3). That is three of the eight `downsampling_*` impulse
  programs; the coopiano blocks never read the streams.

Qualification (§8):

1. **Unit tests.** 15 in `crates/fir/src/outline/tests.rs`, all passing:
   the unchanged module under the budget, largest-first selection with value,
   reference and passed-on reference parameters, the `if` left in the caller,
   the FIR verifier, determinism, the minimum, the four refusals, the seven
   mutations each rejected by its rule (closed: `ByValueWrite`,
   `MissingParam`, `ShadowParam`, `WrongIndex`; calls: `SwapArgs`,
   `CallTwice`; inline-back: `DropStatement`), and the unmutated producer
   accepted.
2. **Corpus.** `crates/compiler/tests/fir_outline_corpus.rs`: 235 hermetic
   programs of `tests/corpus` (none has a `then`/`else` block), 53 of
   `tests/impulse-tests/dsp` (5 with blocks, 6 functions outlined) and three
   inline `ondemand` programs, every block outlined: checker accepted, the
   FIR verifier's errors unchanged, Mojo emitted scalar and batch.
3. **Emitter.** Six tests in `crates/codegen/src/backends/mojo/tests.rs`: the
   method and its call, a `mut` reference printed as the caller's variable,
   no `@always_inline`, a program under the budget identical with and
   without the option, a large program outlined by default (and a block that
   is nearly all of its method left whole), packed `Float32`, `Int32` and
   `Float64` tables, a 255-entry table left as a literal, `packed_tables`
   off.
4. **Codegen identity.** `-lang mojo` single and double over the 646 `.dsp`
   of `tests/` and `examples/`: 1,199 outputs identical to the base build
   (`0d7e4a9b`), 83 failing on both sides, 10 different: 4 programs over the
   budget, outlined (`ondemand_fft_framed_128`, `ondemand_fft_framed_256`,
   `ondemand_stft_denoiser_1024`, `ddsp_rad_harmonic_spectral_frame`), and
   `gpu_bell_reference`, whose two tables are packed. Batch
   (`mojo_batch`, with and without `--no-registers`) over the corpus GPU
   voices and the impulse programs: 286 identical, `gpu_bell_reference`
   different (packed), 2 failing on both sides.
5. **Numeric.** The five different programs built through the Mojo impulse
   harness (`-double`), base against new: identical output, 15,003 lines
   each. Mojo build time, base → new: `ondemand_fft_framed_128` 4.8 → 4.3 s,
   `ondemand_fft_framed_256` 20.1 → 16.4 s, `ddsp_rad_harmonic_spectral_frame`
   48.0 → 43.8 s, `ondemand_stft_denoiser_1024` 629 → 428 s (`compute`
   down to 1,100 lines, but its one outlined block, the `ondemand` body, is
   52,000 straight-line lines: phase 2's case),
   `gpu_bell_reference` 3.9 → 3.1 s (packed tables). The coopiano voices on
   the GPU, against the scripted P0 binaries (same sources, the scripted
   build from faust-rs `2ecf66f6`): the plain voice, 16 keys for 2.048 s
   (`results/fit` G0.4 spec), is bit-identical, 1,572,864 samples of
   1,572,864; the one-tangent voice, 36 tangent instances for 1.024 s (G0.5
   spec), is bit-identical too, 3,538,944 samples of 3,538,944. Metal took
   32 and 100 minutes to compile the two kernels at their first launch
   (P0: about 20 and 90, with fewer compilations running at once). The one-tangent voice was not benched: a second launch found its
   kernel gone from Metal's cache (P0 saw the same when compilations
   overlap) and would have compiled it again.
6. **Gates.** `cargo fmt`, clippy `-D warnings`, the workspace tests (3,619
   passed, 0 failed), the five structure gates (with the code graphs
   regenerated: `fir::outline` and `PACKED_TABLE_MIN_ENTRIES` are new public
   items), `golden-check` (306/306) and `compile-budget-check` (OK; it ran on
   this Mac, whose calibration DSP usually measures under the floor, while
   three Metal compilations loaded the machine). The baskets do not reach the
   Mojo backend, the only caller of the pass.

The coopiano voices, generated as its Makefile does (`mojo_batch
--no-registers`), built without `fit/mojo_tables.py` and
`fit/mojo_outline.py` (Apple M5, Mojo 1.3.0.dev2026110705, `-O3`):

| | scripted (P0) | this pass |
|---|---|---|
| plain voice (`fit/voice.dsp`) | 90 s, 3.0 GB | 70 s, 4.6 GB |
| one-tangent voice (`fit/voice_dir.dsp`) | 145 s, 12.4 GB | 162 s, 8.7 GB (timed during a Metal compilation) |

Both have 51 blocks outlined from `compute`, as the script had; the largest
is 2,207 statements (6,229 in the one-tangent voice). The pass and its
checker take 0.47 s and 0.68 s of the plain voice's 10 s faust-rs
compilation, 1.1 s and 1.7 s of the one-tangent voice's 15 s; most of it is
copying the module into a fresh store (the producer once, the checker twice).

**State promotion (§7).** With promotion on (`mojo_batch` without
`--no-registers`), the plain voice's `compute` carries 4,642 promoted
locals; the largest outlined block takes 1,343 parameters, 576 of them by
reference. It builds in 120 s and 4.3 GB, where P0 found the voice
unbuildable with promotion. Its GPU output is not bit-identical to the
arena build's but within f32 rounding (16 keys: relative L2 error 2.6e-7 to
5.4e-6, −105 to −132 dB; once the state is in locals, Mojo contracts other
products into FMAs).

GPU throughput of the plain voice, 1.024 s of audio, one run each, after a
warm-up launch showed each kernel in Metal's cache (instance-seconds of audio
per second of wall time; ten CPU threads: 175.1, P0):

| build | 1,024 instances | 4,096 instances |
|---|---|---|
| scripted P0 binary | 29.0 s (36.2) | 54.1 s (77.5) |
| outlined, state in the arenas (`--no-registers`) | 22.3 s (47.0) | 36.5 s (114.8) |
| outlined, state promoted (default) | 11.5 s (91.5) | 60.2 s (69.6) |

The outlined build beats the scripted one at the same arithmetic (the
blocks' methods run only when their clock fires, where the script called all
51 at every sample). Promotion doubles the throughput at 1,024 instances and
loses at 4,096, where the 4,642 promoted values of each thread presumably
spill to memory and lower occupancy; not analysed further. The best GPU
figure is now 1.5 times below the ten CPU threads, against 2.4 times in P0.

Public API, Rust-only (`adapted`, no C++ counterpart):
`fir::outline::{outline_large_blocks, OutlinePolicy, OutlineReport,
OutlinedFunction, RefusedBlock, OutlineError, DEFAULT_BUDGET,
DEFAULT_MIN_BLOCK}`, `fir::outline::check::{check_outlining,
OutlineViolation}`, `MojoOptions::outline`, `MojoOptions::packed_tables` and
`codegen::backends::mojo::PACKED_TABLE_MIN_ENTRIES`. Registry: DIFF-BACK-010
extended (the Mojo output differs from the upstream pull request's in these
two ways). Memo: `porting/MEMOIZATION.md` §2.31.

Not done in phase 1 (phase 2 is §10):

- a block that is nearly all of its method, such as the 21,926-statement
  block of the 12-seed `fad` voice, is not split;
- blocks reading a pointer local are not outlined.

## 10. Outcome of phase 2 (2026-11-09)

Implemented as §3.4 planned, in `crates/fir/src/outline/split.rs` (producer)
and `check.rs` (the spills rule, the slots in inline-back).
`OutlinePolicy::split_budget` (default 8,000) turns it on; `mojo_batch`
gains `--outline-split-budget`.

Corrections to the plan from the implementation:

- **A local that may not escape stays in the caller.** As first written,
  a cut was refused wherever a non-single-assignment local crossed it. On the
  12-seed voice that left `compute` at 19,317 lines: `iTemp` values declared
  at the top of the sample loop body are passed to the 51 `ondemand` methods
  spread through it, so no cut between them was allowed. Now an item that
  declares such a local (stored into later, passed to an outlined function,
  an array) and is used after it is a barrier: it stays in the caller and
  the runs reach the local as an ordinary parameter. `compute` is then
  split like the rest.
- **An argument of an outlined call must name its variable.** The first
  corpus run of the producer was rejected by the checker: a statement left in
  the caller held a phase-1 call whose argument the slot rewrite had turned
  into a slot read. Such a local is now pinned (not single-assignment), so
  it never escapes. The checker caught the defect before anything was
  emitted (fail closed).
- **Runs are not split again** (`Added::is_run`): a run is at most a budget
  long, and the checker can then look for the trailing spill stores at the
  top level of a run's raw body.
- **Mutations.** `SpillWrongSlot` first read slot `k + 1`, often stored by a
  later run, so the spills rule (read before store) rejected it before
  inline-back. It now reads an already-stored neighbour (`k - 1`, or `k + 1`
  for slot 0), the defect inline-back exists to catch. `SpillMutable` also
  bypasses the barrier rule above, otherwise it never fires. The three
  mutations are rejected by their rules (spills, inline-back, spills).
- **Checker cost.** The single-assignment test first walked the block once
  per spilled local: 120 s on `ondemand_stft_denoiser_1024` (32,343 spilled
  locals). One walk for all of them: 4.4 s for the whole compilation,
  against 3.1 s without the pass.
- **`mojo_batch --double`** set the emitter's precision but not the
  compiler's, so the computation stayed single-precision with `F64` names.
  Found by the impulse lane below; the example now sets both (no other
  caller used `--double`: the GPU has no f64).

Qualification:

1. **Unit tests.** Six more in `crates/fir/src/outline/tests.rs` (21 in
   all): a 60-statement sample loop body cut into runs through one spill
   array, the mutable local kept in the caller, the output write (a pointer
   local) left as a barrier, the FIR verifier clean, a function under the
   split budget unchanged, and `SpillMutable` (spills), `SpillWrongSlot`
   (inline-back) and `SpillEarly` (spills) rejected.
2. **Corpus.** `fir_outline_corpus.rs` runs every program a second time
   with `budget` and `split_budget` 8: 318 run functions over
   `tests/corpus` and 244 over `tests/impulse-tests/dsp`, every output
   accepted by the checker and the FIR verifier and emitted as Mojo, scalar
   and batch.
3. **Emitter.** A run receives the spill array as `mut compute_spill0:
   Arr[F32, n]`, stores its escapes at its end and reads earlier ones as
   `compute_spill0[S32(k)]`; the caller declares the array zero-filled.
4. **Codegen identity.** The same 1,199 identical outputs as phase 1; of
   the 10 different ones, only `ondemand_stft_denoiser_1024` (its 52,361-
   statement block cut into 43 runs) and `ddsp_rad_harmonic_spectral_frame`
   (an 8,000-plus-statement `instanceConstants` cut into 9 runs) changed
   from phase 1.
5. **Numeric.** `ondemand_stft_denoiser_1024` through the Mojo impulse
   harness: output identical to the base build's (15,003 lines). The
   impulse lane `make mojo-outline` (every program cut into runs of at most
   8 statements, `-double`, against the C++ reference): 132/132 programs
   (`sound` excluded, as in `make mojo`), on Mojo 1.3.0.dev2026110705. `APF`
   and `HPF`, built with `--fp-mode contract=off` and `fast`, give the same
   output whole and split.
6. **The 12-seed `fad` voice** (`faust-coopiano/fit/voice_fad.dsp`,
   `mojo_batch --no-registers`): 181 methods, the largest 6,460 lines (an
   `ondemand` block under the split budget); `instance_clear` (31,465
   lines), `compute` and the two large `ondemand` bodies are cut into runs,
   escapes in six spill arrays (the largest 9,991 `F32`). Mojo builds it in
   633 s and 15.6 GB (timed beside two other Mojo builds); it did not build
   at all in P0. Not run on the GPU: Metal would compile its kernel for
   hours.

7. **Gates.** fmt, clippy `-D warnings`, the workspace tests (3,626
   passed), the five structure gates (code graphs regenerated for
   `DEFAULT_SPLIT_BUDGET`), `golden-check` 306/306; `compile-budget-check`
   refused to measure on the idle Mac (calibration under its floor).

**What the pass does not fix.** `ondemand_stft_denoiser_1024` now builds in
384 s (base 629 s, phase 1 428 s), not far below. Its runs cost about 140 s;
with every run emptied the unit still takes 247 s, and 227 s with
`instance_clear` emptied too. The rest is the struct itself: 2,080 fields
initialised one by one in a 2,082-line `__init__`, which is the emitter's
text, not a FIR function the pass can split. Splitting `__init__` (or
grouping the fields) is the next lever for programs with that much state.
