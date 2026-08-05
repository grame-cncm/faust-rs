# SIGGEN table initialization through generated sub-modules — implementation specification

Date: 2026-08-05
Status: S0-S3 done; S4a (`cpp`/`c` nested emission) is next
Scope: initial content of `rdtable` / `rwtable` tables (`SIGWRTBL(size, SIGGEN(g), …)`)

## 1. Objective

Replace the compile-time evaluation of table generators
(`crates/transform/src/signal_fir/siggen.rs`) by the C++ reference mechanism:
compile the generator signal into its **own FIR sub-module** (`signal2Container`
in C++) whose `fill` function computes the table content **at initialization
time**, and call it from the DSP lifecycle.

Three independent motivations, all measured on this repository at
`2cffc4a3` against Faust C++ `2.87.1`:

1. **Coverage.** Generators that depend on the sample rate or on foreign
   functions cannot be folded at compile time and are rejected today.
2. **Emitted-code size.** Folding a 65536-entry table produces a ~1.4 MB source
   file where the reference emits 4 KB.
3. **Semantic fidelity.** The reference computes the table with the target's
   own float arithmetic at the target's sample rate; folding computes it in
   `f64` on the host at compile time.

The current folding behavior is kept as a permanent, explicitly selected mode
(`--table-init const`, §5.10): it makes the migration bisectable and
backend-by-backend, and it stays useful on its own for targets that want fully
folded tables.

Four decisions are settled and shape the plan (§11); none remains open. `cpp`
and `c` emit a real nested class rather than an inlined body; `--table-init
const` is a permanently supported mode rather than a scaffold; generated tables
take the upstream names `{i|f}tbl{k}[{Sub}]`; and `runtime` mode gives **every**
generator a sub-module, with no folded fast path. Together they make the
`cpp`/`c` output diffable against the reference compiler as text, for the whole
file rather than up to the first foldable generator.

Out of scope: the C++ memory manager (`-mem`), the `-it` inline-table option,
soundfiles, and anything that changes the `SIGWAVEFORM` literal path.

## 2. Baseline

### 2.1 Current code path

`SIGRDTBL` / `SIGWRTBL` lowering lives in
`crates/transform/src/signal_fir/module/tables.rs`:

| Function | Role today |
|---|---|
| `resolve_table` | classifies `SIGWAVEFORM`, read-only `SIGWRTBL(size, gen, nil, nil)`, writable `SIGWRTBL(size, gen, widx, wsig)` |
| `ensure_readonly_table` | expands the generator, declares `Static` table **with a full initializer list** |
| `ensure_wrtbl_table` | expands the generator, declares a `Struct` table and registers a per-instance copy |
| `expand_generator_values` | constant/waveform fast paths, otherwise calls `interpret_generator` |
| `table_size_from_sig` | requires a constant `Int` size |

`interpret_generator` (`siggen.rs`) re-prepares the generator signal through
`crate::signal_prepare::prepare_signals_for_fir` and runs a small step
interpreter over it (recursion groups, `Delay1`, `Delay`, `Prefix`, math,
nested `RdTbl`). It rejects `Input`, UI widgets, bargraphs, soundfiles,
`FConst`, `FVar` and `FFun`.

`register_constant_table_init`
(`crates/transform/src/signal_fir/module/state.rs:504`) then materializes the
per-instance seed for writable tables: unrolled stores up to 256 elements,
otherwise a `Static` companion table plus a copy loop.

The checked vector path duplicates the same folding in
`crates/transform/src/signal_fir/vector/lower/signal.rs`
(`ensure_readonly_table`, `table_initializers`).

### 2.2 Measured consequences

Reference probes, `faust 2.87.1` versus `faust-rs 2cffc4a3`, `-lang cpp`:

| DSP | C++ bytes | faust-rs bytes | ratio |
|---|---|---|---|
| `process = os.osc(440);` (table 65536) | 4 261 | 1 420 238 | 333× |
| `rwtable(4096, sin(…), …)` | 4 189 | 91 609 | 22× |

Compile wall time for the first case: 0.030 s (C++) versus 0.206 s (faust-rs).
For the writable case the content is additionally stored **twice**: once in the
`fTbl…Init` static companion array and once in the per-instance struct table.

### 2.3 Functional gaps

Three programs that the reference compiles and `faust-rs` rejects with
`FRS-SFIR-0004`:

1. `tests/impulse-tests/dsp/subcontainer1.dsp` — the canonical upstream test
   for sample-rate-dependent table content (`rdtable(100, ma.SR, ba.time%100)`).
   This is the one remaining transform error of the impulse corpus.
2. Any sample-rate-dependent generator, e.g.
   `rdtable(1024, exp(-float(ba.time)/ma.SR), …)`. The reference compiles the
   `1/min(192000, max(1, fSampleRate))` constant **inside the sub-container's**
   `instanceInit`.
3. Any `ffunction` inside table content. The reference emits
   `table[i1] = myfun(static_cast<float>(iRec0[1]));` in the fill loop.

UI widgets inside table content are **not** a gap: the reference type checker
rejects them before code generation (`ERROR : checkInit failed for type RSESN`),
so both compilers refuse them, and the sub-module never has to model UI.

### 2.4 One behavior the current path gets right and C++ does not

For a **nested** generated table — a generator that itself reads a generated
table — upstream `2.87.1` declares the inner table
(`static float ftbl0mydspSIG0SIG0[64];`) but never emits its sub-container and
never fills it, so the inner table stays zero. `faust-rs` folds the nested case
correctly today (`process = rdtable(64, rdtable(64, 0.5*float(ba.time), …), …)`
yields `{0, 0.5, 1.0, …}`).

The port must therefore be **recursive** rather than a literal transcription of
the C++ control flow; §5.5 and §8 make this an explicit, tested requirement.

## 3. Reference model (C++)

Pins: `/Users/letz/Developpements/faust` at `3db77f58`, binary `2.87.1`.
Sources: `compiler/generator/instructions_compiler.cpp`,
`compiler/generator/code_container.{hh,cpp}`.

### 3.1 Sub-container creation

```cpp
CodeContainer* InstructionsCompiler::signal2Container(const string& name, Tree sig)
{
    ::Type t = getCertifiedSigType(sig);
    CodeContainer* container = fContainer->createScalarContainer(name, t->nature());
    InstructionsCompiler C(container);
    C.compileSingleSignal(sig);           // per-language compiler variants elided
    return container;
}

void InstructionsCompiler::compileSingleSignal(Tree sig)
{
    sig = prepare2(sig);                  // same normalization as the main program
    pushComputeDSPMethod(IB::genStoreArrayFunArgsVar(
        fContainer->getTableName(), getCurrentLoopIndex(), CS(sig)));
    …
}
```

The sub-container is an ordinary scalar container with 0 inputs, 1 output, whose
sample loop stores into the `table` **function argument** instead of an output
buffer. Its nature (`kInt` / `kReal`) selects the fill argument type.

`generateInstanceInitFun` and `generateFillFun`
(`code_container.cpp:1008` / `:1027`) expose it as two methods:

```cpp
void instanceInit<Sub>(int sample_rate);      // fInit + fPostInit + fResetUI + fClear
void fill<Sub>(int count, <int|float>* table); // fComputeBlock + scalar loop
```

### 3.2 Read-only table (`rdtable`)

`generateRDTbl` → `generateStaticTable` → `generateStaticSigGen`. Emitted shape:

```cpp
class mydspSIG0 { …state…
    void instanceInitmydspSIG0(int sample_rate) { fSampleRate = sample_rate; …clear… }
    void fillmydspSIG0(int count, float* table) { for (…) table[i1] = …; }
};
static mydspSIG0* newmydspSIG0() { return new mydspSIG0(); }
static void deletemydspSIG0(mydspSIG0* dsp) { delete dsp; }

static float ftbl0mydspSIG0[65536];          // no initializer, not const

static void classInit(int sample_rate) {
    mydspSIG0* sig0 = newmydspSIG0();
    sig0->instanceInitmydspSIG0(sample_rate);
    sig0->fillmydspSIG0(65536, ftbl0mydspSIG0);
    deletemydspSIG0(sig0);
}
```

Consequences that the port inherits: the table is file-scope but **mutable**,
and it is **refilled on every `init(sample_rate)`**, which is what makes
sample-rate-dependent content correct.

### 3.3 Writable table (`rwtable`)

`generateWRTbl` → `generateTable` → `generateSigGen`. The table is a struct
field and the same two calls move to `instanceConstants`:

```cpp
float ftbl0[4096];                           // DSP struct field
virtual void instanceConstants(int sample_rate) {
    fSampleRate = sample_rate;
    mydspSIG0* sig0 = newmydspSIG0();
    sig0->instanceInitmydspSIG0(sample_rate);
    sig0->fillmydspSIG0(4096, ftbl0);
    deletemydspSIG0(sig0);
    …
}
```

`generateTable` / `generateStaticTable` also handle the case where the *same*
generator feeds both classes by reusing the already-compiled sub-container and
only adding the missing declaration/call pair.

### 3.4 Two emission shapes from one representation

Textual object-oriented backends (`cpp`, `c`, `rust`, `julia`, `asc`, `cmajor`,
`codebox`) emit the nested container. Flat backends inline it instead:

```cpp
BlockInst* inlined = inlineSubcontainersFunCalls(fStaticInitInstructions);
```

`inlineSubcontainersFunCalls` (`code_container.cpp:782`) renames `sig` to `dsp`,
drops the allocation, and inlines `instanceInit<Sub>` and `fill<Sub>`;
`mergeSubContainers` (`code_container.cpp:572`) merges the sub-container's
declarations into the main struct. This is used by `interp`, `wasm`, `jsfx`,
`codebox` and `julia`. In those backends `classInit` takes the `dsp` pointer and
is instance-scoped.

Even a literal `waveform` generator gets a sub-container upstream:
`rdtable(5, waveform{…}, i)` emits `fmydspSIG0Wave0[5]` inside the
sub-container plus a fill loop that copies it. The port reproduces this in
`runtime` mode (§5.7, option B); only `--table-init const` folds it.

### 3.5 Deliberately not ported

`gMemoryManager` allocation variants, `gInlineTable` (`-it`), the
Rust/Julia/AssemblyScript `delete` "HACK" branches, and the sub-container UI
merge (`mergeSubContainers` inserting sub-UI groups) — table content cannot
contain UI.

## 4. Gap analysis against the Rust pipeline

### 4.1 FIR model (`crates/fir`)

| Need | Status |
|---|---|
| Table declared with a size but no initializer | **missing** in the `static_decls` path: `emit_static_tables` (`c_family.rs:446`) only walks `DeclareTable`, whose length is `values.len()`. `DeclareVar` with `FirType::Array(elem, n)` and `init: None` already renders correctly (`emit_named_type`, `cpp/mod.rs:1008`) and is already predeclared by the interpreter (`interp/compiler.rs:539`). |
| A `staticInit` lifecycle function | **partially present**: the interpreter backend already reserves the slot (`interp/mod.rs:320`, `"staticInit" → static_init_block`), but `build_module` never emits such a function and `cpp` hardcodes an empty `classInit` (`cpp/mod.rs:399`). |
| A sub-module node | **missing**. `FirMatch::Module` has exactly seven fields (`builder.rs:621`) and no container list. |
| Cross-store node import | **available**: `TreeArena::clone_subtree_from` (`crates/tlib/src/arena.rs:474`) already re-interns subtrees, tags included. |

### 4.2 Transform (`crates/transform`)

`SignalToFirLower` owns its `FirStore` (`module/setup.rs:124`), so a nested
`build_module` call produces a separate store whose root must be imported. There
is no `static_init` bucket in `ModuleSections` (`module/state.rs:58`). The
output store `lower_output_signal` (`module/core_lowering.rs:370`) writes to
`outputN[i0]` with a `FaustFloat` cast; a fill module must write to
`table[i0]` with no cast.

### 4.3 Backends

Real emitters: `cpp`, `c`, `rust`, `julia`, `asc`, `cmajor`, `codebox`, `wasm`,
`interp`, `cranelift`. `csharp`, `dlang`, `jax`, `jsfx`, `llvm`, `sdf3`, `vhdl`
are stubs and need nothing. `cmajor` already documents its intended
sub-container mapping in
`porting/cmajor-backend-port-and-test-plan-2026-08-04-en.md` §4.5, and the
lifecycle rules of `porting/backend-lifecycle-contract-en.md` (`init` calls
`classInit` before `instanceInit`) constrain where the fill call may live.

## 5. Target design

### 5.1 Contract

After the port, in `runtime` mode, for **every** `SIGWRTBL(size, SIGGEN(g), …)`
without exception for the payload shape (§5.7):

- **C1** — `g` is compiled into a FIR sub-module with 0 inputs, 1 output, its
  own state, an `instanceInit<Sub>(sample_rate)` and a
  `fill<Sub>(count, table)` function.
- **C2** — the table is declared with its size and **without** an initializer:
  file-scope non-const for `rdtable`, DSP-struct field for `rwtable`.
- **C3** — the fill call sits in `staticInit` (→ backend `classInit`) for
  `rdtable` and in `instanceConstants` for `rwtable`.
- **C4** — the generator's sample-rate constants are computed in
  `instanceInit<Sub>` from the `sample_rate` argument; recompiling is never
  required when the host changes the sample rate.
- **C5** — nested generators are compiled recursively; an inner generated table
  is filled before the outer fill that reads it.
- **C6** — no table read in `compute` can observe an unfilled table under the
  lifecycle contract.
- **C7** — a `SIGWAVEFORM` used **directly as a signal** is untouched: still a
  `const static` initializer list plus its cycling `_idx` field
  (`lower_waveform`). A `SIGWAVEFORM` used as *table content* is a `SIGGEN`
  payload and follows C1: the sub-module owns the literal table and its fill
  loop copies it out, as upstream does.

### 5.2 FIR extension A — sized tables without initializer

Reuse `DeclareVar { name, typ: FirType::Array(elem, n), access, init: None }`.
No new node kind. Required changes:

- `c_family::emit_static_tables` gains a `DeclareVar` arm rendering
  `static <T> <name>[<n>];` (no `const`, no initializer) and keeps the existing
  `DeclareTable` arm for literal waveform tables.
- Each non-C-family backend's static-declaration emitter gains the same arm
  (`asc`, `julia`, `rust`, `cmajor`, `codebox`).
- `fir::checker` gains rule **FIR-SM01**: a `DeclareVar(Array)` in `static_decls`
  or in `dsp_struct` that is read by `compute` must be written by a fill site
  reachable from `staticInit` or `instanceConstants`.

### 5.3 FIR extension B — sub-module node

New node `FIR_SUB_MODULE_TAG` decoding to:

```rust
FirMatch::SubModule {
    name: String,          // "mydspSIG0"
    elem_type: FirType,    // Int32 | Float32 | Float64 — the fill element type
    dsp_struct: FirId,     // Block of DeclareVar (its own state + fSampleRate)
    static_decls: FirId,   // Block — waveform tables owned by the generator
    globals: FirId,        // Block — math prototypes it needs
    functions: FirId,      // Block: instanceInit<Sub>, fill<Sub>
    sub_modules: FirId,    // Block of SubModule — recursion (C5)
}
```

`FirMatch::Module` gains one field, `sub_modules: FirId` (a `Block` of
`SubModule`), so `b.module(...)` takes eight arguments. All construction sites
(backends' unit tests, `codegen/src/fixtures.rs`, `pv_slice.rs`) are updated in
the same commit; there is no dual-arity matcher.

Checker rules:

- **FIR-SM02** — a `SubModule`'s `functions` block contains exactly
  `instanceInit<name>(sample_rate)` and `fill<name>(count, table)`, both
  `Void`-returning, and no `compute`; the module I/O arity contract
  (`checker.rs`, `check_compute_io_arity_contract`) does not apply to it.
- **FIR-SM03** — `fill<name>` writes only to its `table` `FunArgs` argument and
  to its own state; it never touches the parent struct.
- **FIR-SM04** — sub-module names are unique within a module and stable across
  two compilations of the same program (emission determinism, as in
  `porting/scalar-emission-determinism-plan-2026-07-20-en.md`).
- **FIR-SM05** — a sub-module owning nested generators calls each of their
  fills from its own `instanceInit`. Added during S2, after the first producer
  reproduced the upstream defect of §2.4 by assembling a sub-module's
  `instanceInit` from constants/reset/clear only, silently dropping the nested
  fill. A sub-module has no `classInit`, so nested fills have nowhere else to
  go.

### 5.4 FIR extension C — `staticInit`

`build_module` emits, when and only when a static fill site exists:

```text
declare_fun("staticInit", Fun{args:[Ptr(Obj), Int32], ret: Void},
            [dsp, sample_rate], Some(body), false)
```

placed **first** in the `functions` block. Backends map it to `classInit`. The
`dsp` argument is present for the flat backends (§5.8) and is unused — and
therefore droppable — by backends that emit a `static` `classInit`.

Lifecycle invariants of `porting/backend-lifecycle-contract-en.md` are
unchanged: `init` calls `classInit` then `instanceInit`; `instanceInit` never
calls `classInit`.

### 5.5 Transform — generator sub-lowering

New module `crates/transform/src/signal_fir/module/subcontainer.rs`:

```rust
pub(super) struct GeneratorSubModule {
    pub name: String,        // "{module_name}SIG{k}"
    pub elem_type: FirType,
    pub node: FirId,         // SubModule node, already imported in the parent store
    pub size: usize,
}

pub(super) fn build_generator_submodule(
    lower: &mut SignalToFirLower<'_>,
    generator: SigId,        // the SIGGEN payload
    size: usize,
) -> Result<GeneratorSubModule, SignalFirError>;
```

Steps, mirroring `signal2Container` + `compileSingleSignal`:

1. Strip `SigMatch::Gen` and prepare the payload with
   `prepare_signals_for_fir_verified(arena, &[g], &UiProgram::empty())` — the
   same call the interpreter already makes, so the normalization contract is
   unchanged.
2. Call a new `build_fill_module` entry in `module/build.rs`, a thin variant of
   `build_module` parameterized by an output sink
   `OutputSink::{Buffers, Table { name: "table", elem_type }}`. Only
   `lower_output_signal` (`core_lowering.rs:370`) branches on it: the table sink
   emits `store_table("table", FunArgs, i0, value)` with **no** `FaustFloat`
   cast. Everything else — recursion carriers, delay strategies, CSE,
   scheduling, clocked regions — is reused untouched.
3. Assemble the sub-module: `instanceInit<Sub>` = `constants ++ reset_ui ++
   clear` sections; `fill<Sub>` = control statements ++ sample loop over
   `count`.
4. Import the resulting root into the parent store with
   `FirStore::import_from(&child_store, root)` — a new thin wrapper over
   `TreeArena::clone_subtree_from`.
5. Recurse: a generator that contains another `SIGWRTBL(_, SIGGEN(_), nil, nil)`
   produces a nested `SubModule` plus its own static table and fill call, placed
   in the sub-module's `static_decls` / `sub_modules` and called at the top of
   `instanceInit<Sub>` (C5). Emission order is deepest-first.

`tables.rs` changes:

- `ensure_readonly_table` — declare
  `DeclareVar(name, Array(elem, size), Static, None)` into
  `sections.static_declarations`, build the sub-module, and push the
  allocation/init/fill triple into a **new** `sections.static_init_statements`
  bucket.
- `ensure_wrtbl_table` — same, with the declaration going to
  `struct_declarations` and the triple to `constants_statements`, replacing the
  `register_constant_table_init` call **on the `runtime` path only**.
  `register_constant_table_init` and its 256-element unroll threshold
  (`state.rs:514`) stay: they remain the seeding mechanism under
  `--table-init const`, which is a permanent mode (§5.10).
- `expand_generator_values` — whole function, all three arms included, moves
  behind the `--table-init const` switch (§5.7, option B). The `runtime` path
  never classifies the payload: it calls `build_generator_submodule`
  unconditionally. `siggen.rs` and `expand_generator_values` are therefore
  permanently supported `const`-mode code, not a transition scaffold.
- Table sharing stays keyed on the producing `SigId`
  (`ui.waveform_tables` / `waveform_table_len` / `table_access_by_sig`), so one
  generator feeding two reads still yields one table and one fill.

### 5.6 Naming

Decided 2026-08-05: adopt the upstream table naming as well, so that the `cpp`
and `c` differential of §8.2 becomes a near-textual diff instead of a structural
one.

| Item | Target | Upstream form |
|---|---|---|
| Sub-module | `{module_name}SIG{k}` | identical (`mydspSIG0`) |
| Fill / init | `fill{Sub}`, `instanceInit{Sub}` | identical |
| Read-only generated table | `{i\|f}tbl{k}{Sub}` | identical (`ftbl0mydspSIG0`) |
| Writable generated table | `{i\|f}tbl{k}` — **no** sub-module suffix | identical (`ftbl0`, `itbl1`) |
| Literal waveform table | `{i\|f}{Container}Wave{j}` + `_idx` companion | identical (`fmydspWave0`, `fmydspSIG0Wave0`) |
| Sub-module local state | existing generators (`iRec…`, `fVec…`) with sub-module-local counters | same scheme, numbering still diverges |

Rules, verified against `faust 2.87.1`:

1. **One shared counter.** `k` comes from a single fresh-ID counter for the
   `tbl` prefix, shared by integer and real tables (C++ `getTypedNames` builds
   `"i"|"f"` + `getFreshID("tbl")`, `instructions_compiler.cpp:1040`). A program
   with an int table then a real table yields `itbl0` then `ftbl1`, never two
   zeros. The type letter is a prefix, not part of the counter key.
2. **The suffix marks the static class only.** `generateStaticTable` appends the
   filling sub-module's name (`vname += tablename`), `generateTable` does not.
   So `rdtable` tables are `ftbl0mydspSIG0` at file scope and `rwtable` tables
   are `ftbl0` as struct fields. The suffix names the sub-module that *fills*
   the table, which is why a nested generator produces `ftbl0mydspSIG0SIG0`
   (inner) alongside `ftbl1mydspSIG0` (outer).
3. **Waveform tables keep their own namespace**, prefixed by the container that
   owns them, so a waveform inside a generator is `f{Sub}Wave{j}`.

Consequences to plan for:

- The counter is allocation-ordered, not `SigId`-derived, so table names now
  depend on emission order. This strengthens invariant I4: the emission
  determinism gate must cover table naming explicitly, not only statement
  order.
- Every current `fTbl{sigid}` / `iTbl{sigid}` occurrence changes, in both
  `--table-init` modes. The rename is mechanical but wide: `module/tables.rs`,
  the `{name}Init` companion in `state.rs`, and every test or snapshot that
  matches on table names (`transform` table tests, backend emission tests,
  `tests/impulse-tests` structural checks). It is done once, in its own commit
  at the start of S2, before any behavior change, so that the naming diff and
  the semantic diff never mix.
- The checked vector path currently uses `fVecTbl{sigid}` / `iVecTbl{sigid}` /
  `fVecWave{sigid}` (`vector/lower/signal.rs`). Those follow the same rules in
  S6; there is no reason for scalar and vector output to name the same table
  differently.

**Known residual after the S2 rename (2026-08-05).** Waveform tables now match
the reference exactly, names and `_idx` companions included
(`fmydspWave0[3]` / `imydspWave1[3]`). Generated tables match in *form* but not
always in *counter order*: on `f13_mixed_type_tables` upstream allocates
`itbl0`(64), `ftbl1`(32), `itbl2`(16) in program order while faust-rs allocates
`ftbl0`(32), `itbl1`(64), `itbl2`(16). The counter is allocation-ordered in both
compilers; what differs is the order in which each one first materializes a
table during lowering. This is stable across runs — the emission-determinism
requirement (I4) is met — and it is a pure naming residual with no semantic
effect. Closing it means aligning traversal order with the reference, which is
a separate concern from this port; S4a should record the residual in its diff
rather than treat it as a defect.

### 5.7 No folded fast path in `runtime` mode

Decided 2026-08-05 (option B): in `runtime` mode **every** `SIGGEN` becomes a
sub-module, with no exception for literal `Waveform` or constant `Int`/`Real`
payloads. This is literal upstream parity. Folding survives only under
`--table-init const`, where it applies to exactly the same two arms.

Consequences for the producer:

- The `runtime` path in `tables.rs` has one shape:
  `build_generator_submodule` is called for every `SIGWRTBL(_, SIGGEN(_), …)`,
  with no classification step in front of it.
- `expand_generator_values` and its three arms become `const`-mode-only code,
  reached through the same option switch as `interpret_generator`.
- A waveform used as table content produces a sub-module that owns its own
  `const static` waveform table plus a cycling index, exactly as upstream
  (`fmydspSIG0Wave0` + `fmydspSIG0Wave0_idx`); the fill loop copies it into the
  target table. A waveform used **directly as a signal** is a different node
  and is untouched (C7).
- Table and sub-module counters now advance at the same points as upstream for
  every program, so the `cpp`/`c` text diff of §8.2 stays aligned for the whole
  file rather than up to the first foldable generator.

The rationale, measured (`faust 2.87.1` vs `faust-rs 2cffc4a3`, `-lang cpp`,
emitted bytes):

| Generator | Reference | Folded |
|---|---|---|
| constant, N = 65536 | 3 123 | **395 486** (127×) |
| waveform of 5, N = 5 | 3 083 | **2 178** (0.7×) |
| waveform of 5, N = 4096 | 3 346 | **30 942** (9×) |

Folding only wins when the table length equals the waveform length. The
constant arm is a pre-existing defect independent of this port: it emits `size`
identical literals where the reference emits `table[i1] = 0.5f;` in a loop.

Four axes weighed:

- **Size** — as measured; folding's win is confined to N = M.
- **Constness** — a folded table is `const static`: shareable, ROM-placeable, no
  init phase, no thread-safety question. This is folding's only advantage that
  does not depend on size, and it remains reachable through
  `--table-init const`, which can always fold these two arms.
- **Numerics** — none. Neither arm can depend on the sample rate or on foreign
  state, so both shapes yield identical values, rounded once to `f32`.
  Invariant I5 does not apply here; the mode matrix (§8.2 layer 6) may compare
  `runtime` and `const` output bit-for-bit on these fixtures, in `-single` as
  well as `-double`.
- **Counter alignment** — created by the naming decision of §5.6. Upstream
  advances `SIG{k}` and `tbl{k}` at every sub-container and table it creates,
  so any folded generator would renumber every later table in the same DSP.
  Full parity removes the question.

Accepted cost: `rdtable(M, waveform{…})` gains a class and a fill loop, about
900 bytes on a 3 KB file, and the folded form is one flag away.

### 5.8 Flattening pass

This pass exists for backends that cannot express a nested container. It is
**not** used by `cpp` and `c`, which emit the real nested class (§5.9.1,
decided 2026-08-05).

New `crates/fir/src/subcontainer.rs`:

```rust
pub enum SubModuleStatePolicy {
    /// Sub-module state becomes stack locals of the enclosing function.
    /// Required by backends whose `classInit` is `static`.
    StackLocals,
    /// Sub-module state is merged into the DSP struct with a name prefix.
    /// Port of C++ `mergeSubContainers`; required by heap/flat backends.
    MergedStructFields,
}

pub fn flatten_sub_modules(
    store: &mut FirStore,
    module: FirId,
    policy: SubModuleStatePolicy,
) -> Result<FirId, FirError>;
```

Behavior — the port of `inlineSubcontainersFunCalls` + `DspRenamer`:

1. Walk `sub_modules` deepest-first.
2. Replace each `new<Sub>` / `delete<Sub>` statement pair with nothing.
3. Inline `instanceInit<Sub>(obj, sr)` and `fill<Sub>(obj, count, table)` at
   their call sites, substituting the `table` `FunArgs` reference with the
   caller's table variable and `count` with the constant size.
4. Rewrite state accesses according to the policy, renaming loop variables to
   avoid clashes (C++ `LoopVariableRenamer`).
5. Move the sub-module's `static_decls` and `globals` into the parent's.
6. Drop the now-empty `sub_modules` block.

The pass is pure FIR→FIR and is validated by an independent structural checker
(§6), not by the backend that consumes it.

### 5.9 Backend contract

| Backend | Shape | Work |
|---|---|---|
| `cpp`, `c` | **native nested class** (§5.9.1) | full reference shape: nested class/struct, `new`/`delete` helpers, `getNumInputs`/`getNumOutputs`, `instanceInit<Sub>`, `fill<Sub>`; render `staticInit` as the `classInit` body (replacing the hardcoded empty one at `cpp/mod.rs:399`); `DeclareVar(Array)` arm in `emit_static_tables` |
| `rust`, `julia`, `asc` | native sub-module | same, using the language's struct/impl form; no `delete` |
| `cmajor` | native sub-module | as already specified in the cmajor plan §4.5 (size-suffixed fill names, `Struct&` receiver), per-instance tables |
| `codebox` | flattened, `StackLocals` | it already folds the lifecycle into one entry point |
| `wasm` | flattened, `MergedStructFields` | matches upstream; `classInit` keeps its `dsp` argument |
| `interp` | flattened, `MergedStructFields` | fill bytecode lands in the already-existing `static_init_block` / `init_block`; `compile_static_decls_init_block` keeps handling literal `DeclareTable`s and gains storage predeclaration for uninitialized `DeclareVar(Array)` |
| `cranelift` | flattened, `MergedStructFields` | fill code compiled into the `classInit`/`instanceConstants` JIT functions; removes the current `CodeTooLarge` pressure that motivated the 256-element unroll threshold |

Every backend keeps a hard failure (`FRS-CGEN-*-…`) when it meets a `SubModule`
it has not been migrated to handle; no backend may silently skip a fill call.

### 5.9.1 Nested emission contract for `cpp` and `c`

Decided 2026-08-05: these two backends emit the reference nested container
literally, not a flattened body. The target shapes, taken from `faust 2.87.1`
on `process = os.osc(440);`, are the emission oracle for S4.

`-lang cpp` — nested class before the DSP class, at namespace scope:

```cpp
class mydspSIG0 {
  private:
    int iVec0[2];
    int iRec0[2];
    int fSampleRate;
  public:
    int getNumInputsmydspSIG0() { return 0; }
    int getNumOutputsmydspSIG0() { return 1; }
    void instanceInitmydspSIG0(int sample_rate) { … }
    void fillmydspSIG0(int count, float* table) { … }
};

static mydspSIG0* newmydspSIG0() { return (mydspSIG0*)new mydspSIG0(); }
static void deletemydspSIG0(mydspSIG0* dsp) { delete dsp; }

static float ftbl0mydspSIG0[65536];
```

`-lang c` — struct plus free functions, state reached through `dsp->`:

```c
typedef struct { int iVec0[2]; int iRec0[2]; int fSampleRate; } mydspSIG0;

static mydspSIG0* newmydspSIG0() { return (mydspSIG0*)calloc(1, sizeof(mydspSIG0)); }
static void deletemydspSIG0(mydspSIG0* dsp) { free(dsp); }

static void instanceInitmydspSIG0(mydspSIG0* dsp, int sample_rate) { … }
static void fillmydspSIG0(mydspSIG0* dsp, int count, float* table) { … }

static float ftbl0mydspSIG0[65536];
```

Required emitter work, shared through `c_family.rs` wherever the two languages
already share statement/value emission:

1. **Placement.** Sub-modules are emitted after the header and the literal
   waveform tables, before the DSP class/struct, deepest-first, so a nested
   generator's class precedes the class that calls it. Each sub-module's own
   uninitialized table declaration follows its class, exactly as upstream.
2. **State scope.** `cpp` renders `AccessType::Struct` inside a sub-module as a
   bare field access; `c` renders it as `dsp->field` with the sub-module struct
   as the receiver type. This is the same seam the two backends already use for
   the main DSP struct, so it is a receiver-type change, not a new mechanism.
3. **Allocation.** `new`/`delete` helpers are emitted per sub-module —
   `new`/`delete` for `cpp`, `calloc`/`free` for `c`. `c` therefore needs
   `<stdlib.h>` in its include set whenever a sub-module exists.
4. **`classInit` body.** `staticInit` is rendered inside `classInit`
   (`cpp`: `static void classInit(int sample_rate)`, `c`:
   `void classInit<name>(int sample_rate)`). Because the sub-module object is a
   local of that function, a `static` `classInit` remains valid — this is what
   the nested shape buys over `StackLocals`.
5. **Arity getters.** `getNumInputs<Sub>` / `getNumOutputs<Sub>` are emitted for
   reference parity even though nothing calls them; they are derived from the
   sub-module's fixed 0/1 arity, not from a FIR function.

Structural tests for S4 assert, on the `os.osc(440)` fixture: exactly one
sub-module class, one `new`/`delete` pair, an uninitialized non-`const` table
declaration, one `fill` call inside `classInit` with the literal size `65536`,
and no initializer list anywhere in the file.

### 5.10 Options and interactions

New compiler option, plumbed like `-mcd` (`crates/compiler/src/lib.rs:650`,
`service.rs:91`) into `SignalFirOptions`:

```text
--table-init runtime|const      (default: runtime)
```

- `runtime` — this specification.
- `const` — the compile-time interpreter path (`siggen.rs`). Decided
  2026-08-05: **this mode is permanent**, not a migration scaffold. It is the
  only way to obtain fully folded tables (useful for targets without an
  initialization phase, for embedded ROM placement, and for bisecting numeric
  differences), and it stays available on every backend. When a generator
  cannot be folded it keeps failing with `FRS-SFIR-0004`, with a help line
  pointing at `--table-init runtime`.

Because `const` is permanent, `siggen.rs`, `register_constant_table_init` and
the folded `const static` table shape remain first-class supported code: they
carry their existing tests and gain the fixture matrix of §8.1 run in both
modes. Neither mode may be removed without a follow-up plan.

Interactions:

- `-double` — the sub-module inherits the parent's `RealType`.
- `-vec` — table fill stays outside every vector loop; the checked vector
  lowerer (§S6) either builds the same sub-module or fails closed with a named
  reason. Read-only generated tables carry no compute-time effect
  (`porting/readonly-generated-table-effect-plan-2026-07-17-en.md`); that model
  is unchanged.
- `-os` / `-ec` — fill sites are in `classInit` / `instanceConstants`, outside
  `frame` and `control`; unaffected.
- JSON — with `MergedStructFields`, merged generator state enlarges the reported
  DSP size. `crates/codegen/src/json*` must be checked against the reference for
  at least one generated-table fixture.

## 6. Invariants and independence obligations

Following `porting/` methodology, the producer never validates itself.

| # | Invariant | Independent check |
|---|---|---|
| I1 | Every uninitialized table read in `compute` is filled before use | `fir::checker` rule FIR-SM01, walking lifecycle bodies, independent of `tables.rs` |
| I2 | `fill<Sub>` writes exactly `size` elements, indices `0..size` | structural checker over the fill loop bounds (FIR-SM03) |
| I3 | Sub-module state never aliases parent state | name-domain check after flattening, both policies |
| I4 | Lowering is deterministic, **table names included** | two compilations of the same program produce byte-identical FIR dumps; the gate is extended to assert identical `{i\|f}tbl{k}` assignments, since `k` is now allocation-ordered rather than `SigId`-derived (§5.6) |
| I5 | `runtime` and `const` agree numerically | differential run in `-double`, where the folded `f64` values and the runtime `f64` computation must match bit-for-bit; in `-single` compare against the C++ reference instead, not against the folded path |
| I6 | Nested generators are filled in dependency order | fill-order test on a 2-level fixture (§2.4) |

Rejecting mutations that must turn each check red:

- drop the fill call from `staticInit` → I1 fails;
- change the fill loop bound to `size - 1` → I2 fails;
- reuse a parent state name inside a sub-module → I3 fails;
- seed the sub-module or table counter from a `HashMap` iteration order → I4
  fails;
- append the sub-module suffix to a writable table name → the §8.2 differential
  against the reference fails (§5.6 rule 2);
- reintroduce a waveform or constant fast path on the `runtime` path → the
  waveform-content and constant-content structural tests fail, and the table
  numbering of any later table in the same DSP drifts from the reference
  (§5.7);
- fill the outer table before the inner one → I6 fails.

## 7. Implementation phases

### S0 — Freeze and baseline — **done 2026-08-05**

Reference and current outputs for the §8.1 fixtures, sizes, timings, structural
counts, and the re-measured impulse baseline are frozen under
`porting/generated/siggen-table-init-s0/`
([baseline](generated/siggen-table-init-s0/baseline-2026-08-05-en.md)). No code
changed. Three results feed back into this plan:

- The impulse gates are at **93/93** on `cpp`, `c` and `interp`, not the
  inherited 92/93 · 87/93 · 74/93. §8.2 layer 4 is restated accordingly.
- A thirteenth fixture was added, `f13_mixed_type_tables`: §5.6 rule 1 (one
  `tbl` counter shared by int and real tables) was an inference from
  `getTypedNames` until this probe produced
  `itbl0mydspSIG0` / `ftbl1mydspSIG1` / `itbl2mydspSIG2`.
- The `f08` nesting defect of §2.4 is quantified: the reference declares two
  static tables and emits **one** filler class, leaving the inner table zero.
  `f08` is a regression guard, not a parity target.

### S1 — FIR model — **done 2026-08-05**

`SubModule` node (`FIRST_SUBMODULE`), eighth `Module` field `sub_modules`,
`FirStore::import_from` over `TreeArena::clone_subtree_from`, matcher/dump/
inliner traversal, checker rules FIR-SM01…SM04, and a fail-closed guard in
every backend decoder. `DeclareVar(Array)` in `static_decls` needed no change:
`check_globals` already accepts `Static`/`Global` variable declarations.

Gate met: `cargo test --workspace` green; one accepted `SubModule` fixture plus
four mutated fixtures, each verified to fail when its rule is disabled; one
backend test proving the fail-closed rejection.

Two corrections to this plan came out of the implementation:

- The rule codes are **FIR-SM01…SM04**, not FIR-T01…T04 as first written:
  `FIR-T01`–`T04` are already taken by the table-access rules
  (`checker.rs`, index type, store element type, missing declaration), and
  `FIR-M06` is already taken too. Reusing them would have made the mutation
  tests satisfiable by an unrelated diagnostic — which is exactly what the
  first draft of those tests did before the collision was found.
- `FirBuilder::module` takes `sub_modules: &[FirId]` rather than a pre-built
  block id, so the ~100 existing call sites append `&[]` instead of gaining a
  statement. The clone paths in `fir::inliner` must clone the block's *items*
  (`clone_sub_modules`), never the block node itself: passing the block would
  nest a block inside a block and break every consumer that expects
  `sub_modules` to decode as a block of `SubModule`.

### S2 — Transform producer — **done 2026-08-05**

Landed in two commits, as specified: the naming-only rename first
(`c093e424`), then the producer. `--table-init runtime` compiles every `SIGGEN`
into a sub-module; `const` stays the effective default until S7. The three
fixtures that could not be compiled at all now lower successfully and stop at
the S1 backend guard, which is this phase's intended end state:
`f02_subcontainer1`, `f03_sr_dependent` and `f09_ffunction_gen` return
`FRS-CGEN-CPP-0003` instead of `FRS-SFIR-0004`.

Sub-module counts are as designed: one per generator, one for two reads of the
same generator (`f11`), `SIG0`/`SIG1` for two distinct ones (`f12`), and
`mydspSIG0SIG0` nested inside `mydspSIG0` for `f08`.

Two implementation notes:

- `FirBuilder::module` had to grow no further: the sub-module list is taken
  from the lowering (`std::mem::take(&mut lower.sub_modules)`), not from the
  caller's `FillSpec`. The first version read it from the spec, which is
  always empty at that point, so nested sub-modules were dropped.
- The producer initially reproduced the upstream nesting defect exactly (§2.4):
  a sub-module's `instanceInit` was built from constants/reset/clear, so a
  nested generator's fill call vanished. Fixed by prepending
  `static_init_statements`, and locked by new rule FIR-SM05.

Original phase description:

Opens with the **naming-only commit** of §5.6 — `{i|f}tbl{k}[{Sub}]` everywhere,
no behavior change, all tests updated — so that the rename never mixes with the
semantic diff. Then `OutputSink` in `build_module`, `build_fill_module`,
`module/subcontainer.rs`, `ModuleSections::static_init_statements`, rewritten
`ensure_readonly_table` / `ensure_wrtbl_table`, `--table-init` option with
`const` still the effective default at the end of this phase. Gate:
`cargo test -p transform --lib` unchanged, plus FIR-level tests asserting the
emitted shape for `rdtable`, `rwtable`, sample-rate-dependent, `ffunction`,
waveform-content, constant-content, and nested fixtures — the last two are what
prove option B is actually applied and no payload classification survives on the
`runtime` path.

### S3 — Flattening pass — **done 2026-08-05**

`fir::subcontainer::flatten_sub_modules` with both state policies, plus
`verify_flattened` as the independent structural check (no sub-module left
declared or reachable, no surviving allocation, no call to a vanished entry
point). Tests run the **real S2 producer** and flatten its output rather than a
hand-built fixture, since the pass exists to consume what the producer emits.

Three implementation findings:

- The pass reuses the inliner's hygienic clone engine rather than duplicating
  its traversal. `HygienicCloner`'s `fun_arg_subst` was generalized from
  `name -> name` to `name -> (name, AccessType)`, and a `struct_subst` map
  added: a flattened `fill` writes into the caller's own table, which lives in
  `Static` or `Struct` storage, so the access class has to travel with the
  name instead of being assumed to be `Stack`.
- Sub-modules must be resolved from the **callee name**, not from the receiver
  object. The clone engine hoists a `NewDsp` out of its declaration into a
  statement of its own, so a receiver-based lookup silently missed nested
  allocations; `verify_flattened` caught it.
- Recursion happens on the *spliced* statements: a sub-module's `instanceInit`
  can itself contain an allocate/init/fill triple for a nested generator, so
  inlining is followed by another rewrite pass over the result.

S3 also uncovered an S2 producer bug: fill bodies carried the DSP path's
`output0 = outputs[0]` channel aliases, referencing an `outputs` parameter a
`fill` signature does not have. Fixed by skipping the alias emission when
lowering a fill module.

### S4 — C-family and textual backends

Split in two, because `cpp`/`c` now carry the full nested-class emitter:

- **S4a — `cpp` and `c`.** Nested class/struct emission per §5.9.1: sub-module
  placement, receiver-type seam for state access, `new`/`delete` helpers,
  `classInit` body from `staticInit`, uninitialized static table declarations,
  `<stdlib.h>` include for `c`. Gate: generated sources compile, the structural
  assertions of §5.9.1 hold, lifecycle conformance tests still pass, and the
  `os.osc(440)` fixture drops below 10 KB.
- **S4b — `rust`, `julia`, `asc`, `cmajor`, `codebox`.** Same gates in their own
  language shapes; `codebox` uses the flattened form.

Both sub-phases must keep every S4 fixture passing under `--table-init const`
as well; the two modes are gated together from S4 onwards.

### S5 — Flat backends

`wasm`, `interp`, `cranelift`. Gate: interpreter and JIT numeric outputs match
the `cpp` output of the same program for every S2 fixture.

### S6 — Vector path

`vector/lower/signal.rs`: build the same sub-module, or fail closed with a
stable `FRS-VEC-FALLBACK-…` reason. No silent folding may remain in the vector
lowerer once `--table-init runtime` is the default. Gate: the 132-DSP vector
corpus keeps its certified count.

### S7 — Default switch and qualification

Flip the default to `runtime`, run the full impulse corpus, corpus qualification
in all 16 modes, size/cost report, and a `JOURNAL.md` entry in English. The
`const` mode is qualified in the same run, not dropped: the impulse corpus is
executed in both modes, with the known `FRS-SFIR-0004` rejections of §2.3
recorded as the expected `const`-mode outcome rather than as failures.

## 8. Test plan

### 8.1 Fixture matrix

| Fixture | Property exercised |
|---|---|
| `os.osc(440)` | 65536-entry read-only table, recursive phasor generator |
| `tests/impulse-tests/dsp/subcontainer1.dsp` | `fSamplingFreq` inside the generator (the gap of §2.3) |
| `rdtable(1024, exp(-float(ba.time)/ma.SR), …)` | sample-rate-dependent content, `-single` and `-double` |
| `rwtable(4096, sin(…), …, _, …)` | writable table seeded in `instanceConstants` |
| `rdtable(5, waveform{…}, …)` | waveform as generator content: sub-module owning `f{Sub}Wave0` + copy loop (§5.7); folded only under `const` |
| `waveform{…}` used directly as a signal | untouched folded path with its `_idx` field (C7) |
| `rdtable(65536, 0.5, …)` | constant payload also becomes a sub-module; guards the 127× regression of §5.7 |
| nested `rdtable(rdtable(...))` | recursion and fill order (C5, I6) |
| `ffunction` generator | foreign call inside the fill loop |
| int-typed generator | `iTbl` element type and `int* table` fill signature |
| two reads of one generator | single table, single fill |
| two distinct generators | deterministic `SIG0` / `SIG1` numbering |

### 8.2 Layers

1. **Unit / structural** — FIR shape assertions in `transform` and `fir`; the
   independent checkers of §6 with their rejecting mutations.
2. **Backend source** — per-backend emission tests, including the lifecycle
   conformance tests required by
   `porting/backend-lifecycle-contract-en.md`.
3. **Differential vs C++** — for each fixture, compare generated structure
   against `faust 2.87.1` output: presence of a sub-container (or its inlined
   equivalent), absence of a folded initializer list, fill count, and call
   placement. For `cpp` and `c` this comparison is tightened to the nested-class
   shape of §5.9.1 with upstream table names (§5.6): the sub-module class, its
   two methods, the `new`/`delete` helpers, the table declaration, and the
   `classInit` body should diff against the reference modulo state-variable
   numbering, whitespace and the metadata block. Record the residual diff for
   each fixture in `porting/generated/` — it is the standing measure of how far
   the two compilers have drifted.
4. **Numeric** — `tests/impulse-tests` for `cpp`, `c`, `interp`. S0 re-measured
   the baseline and found **93/93 on all three**, not the cpp 92/93, c 87/93,
   interp 74/93 quoted by earlier documents
   (`porting/generated/siggen-table-init-s0/baseline-2026-08-05-en.md`). The
   target is therefore exact rather than comparative: remove
   `KNOWN_FAIL_all := subcontainer1` from `tests/impulse-tests/known.mk` and its
   row from `KNOWN_FAILURES.md`, take the gate from 93 to 94 cases, and keep all
   three backends at 100%. There are no pre-existing failures for a regression
   to hide behind.
5. **Cost** — emitted-source bytes and compile wall time for the §8.1 fixtures,
   recorded in `porting/generated/` and in the journal entry.
6. **Mode matrix** — every §8.1 fixture is compiled in both `--table-init` modes
   on every migrated backend. `const` must keep producing the folded
   `const static` table for foldable generators and the documented
   `FRS-SFIR-0004` rejection otherwise. This layer is what makes the permanence
   of `const` (§5.10) enforceable rather than aspirational.

## 9. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Numeric drift `f32` runtime fill vs `f64` folded fill | Expected and desired: the runtime fill is the reference behavior. Validate against C++, never against the folded path, in `-single` (I5). |
| A backend silently ignores a `SubModule` and emits a zero table | Hard per-backend error on unhandled `SubModule`; checker rule FIR-SM01 runs before emission. |
| Static tables become non-`const`, losing read-only placement | Matches the reference; document it, and keep literal waveform tables `const` (C7). |
| Thread safety: `classInit` refilling shared static tables | Same exposure as the reference. Do not add locking; state it in the journal entry. |
| Regression on nested generators (§2.4) | C5 + I6 + a dedicated fixture; the fallback `--table-init const` keeps the folded behavior available. |
| Module node arity change breaks many call sites | Single mechanical commit in S1; no dual-arity matcher. |
| Cranelift function-size limits move from init to fill | The fill is a loop, not an unrolled body, so `runtime` removes the pressure entirely. The 256-element unroll threshold in `state.rs:514` is **kept**, because `--table-init const` still needs it (§5.10). |
| Two permanently supported table-init modes double the emission surface | The §8.2 mode matrix gates both on every migrated backend. The split is one option switch in `ensure_readonly_table` / `ensure_wrtbl_table`: `runtime` always builds a sub-module, `const` always calls `expand_generator_values`. Neither path branches on payload shape, and `const` reuses code that already exists and is already tested. |
| Nested-class emission is a genuinely larger `cpp`/`c` change than flattening | S4 is split into S4a (`cpp`/`c`) and S4b; §5.9.1 fixes the target text in advance so the emitter is written against a frozen oracle, and the state-access seam reuses the existing receiver-type mechanism rather than a new one. |
| Table rename touches many tests and snapshots at once | Isolated naming-only commit at the head of S2, no behavior change in it, so a later bisect separates "renamed" from "changed"; the mode matrix runs on it before S2 continues. |
| Allocation-ordered table counter makes names order-sensitive | I4 extended to table naming with its own rejecting mutation; the counter lives next to the existing `NameGen` counters rather than in a fresh ad-hoc global. |

## 10. Completion checklist

- [x] S0 baseline artifacts recorded under
      `porting/generated/siggen-table-init-s0/` (2026-08-05)
- [x] `SubModule` node, sized uninitialized tables, checker rules FIR-SM01…SM05,
      fail-closed backend guards (2026-08-05)
- [x] Upstream table naming `{i|f}tbl{k}[{Sub}]` landed as a standalone commit (2026-08-05)
- [x] `build_fill_module` and `module/subcontainer.rs` with recursion (2026-08-05)
- [ ] `--table-init runtime|const` implemented (2026-08-05); default flips to
      `runtime` in S7, both modes gated
- [x] Flattening pass with both state policies + independent checker (2026-08-05)
- [ ] `cpp`/`c` nested-class emission matching §5.9.1, with its structural tests
- [ ] All ten backends migrated or explicitly failing on `SubModule`
- [ ] Vector path migrated or failing closed with a stable reason
- [ ] `subcontainer1.dsp` passes; no impulse regression
- [ ] `os.osc(440)` emitted source under 10 KB
- [ ] Mode matrix (§8.2 layer 6) green on every migrated backend
- [ ] Journal entry in English

## 11. Decisions

### Settled

- **`cpp`/`c` emission shape** (2026-08-05) — emit the real nested class /
  struct, not the `StackLocals` flattening. Specified in §5.9.1; S4 is split
  into S4a/S4b accordingly. Rationale: the generated code stays structurally
  diffable against the reference compiler, which is the main oracle for these
  two backends, and a `static classInit` remains valid because the sub-module
  object is a local of that function.
- **`--table-init const` lifetime** (2026-08-05) — permanent supported mode, not
  a transition scaffold. `siggen.rs`, `register_constant_table_init` and the
  folded `const static` shape stay first-class and are gated by the §8.2 mode
  matrix. Deleting either mode requires its own plan.
- **Table naming** (2026-08-05) — adopt the upstream form:
  `{i|f}tbl{k}{Sub}` for read-only generated tables, `{i|f}tbl{k}` for writable
  ones, one shared allocation-ordered counter, waveform tables unchanged in
  their own namespace. Specified in §5.6, landed as a standalone rename commit
  at the head of S2. Rationale: combined with the nested class, it makes the
  `cpp`/`c` output diffable against the reference modulo state-variable
  numbering, which turns the §8.2 differential from a structural checklist into
  a text diff whose residual is worth tracking.
- **Folded fast paths** (2026-08-05) — option B, full upstream parity: in
  `runtime` mode every `SIGGEN` becomes a sub-module, with no fast path for
  literal waveform or constant payloads. Specified in §5.7. Rationale: folding
  only wins when table length equals waveform length, it blows up 127× on a
  large constant table, it would renumber tables relative to the reference and
  break the alignment bought by the naming decision, and its one durable
  advantage — a `const` ROM-able table — stays reachable through
  `--table-init const`.

### Still open

None. The plan is fully specified; S0 may start.
