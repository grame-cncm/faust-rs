# `fir::outline`: one shared function for isomorphic outlined blocks — plan (2026-11-10)

Status: planned 2026-11-10, not started. Decisions D1–D4 (§8) are open.

Parent: option C of the study
`porting/cpp-isomorphic-outlined-functions-study-2026-11-10-en.md`; builds
on `--outline` (`porting/cpp-outlining-plan-2026-11-10-en.md`, `0210fee4`)
and the reference promotion
(`porting/fir-outline-reference-promotion-plan-2026-11-10-en.md`,
`47acfb87`).

## 1. Problem

With `--outline`, the 36 × 36 gated-delay program becomes 1,298
`computeOutline<k>` methods, 1,296 of them one per line of the matrix and
identical up to renaming: each touches its own 53 struct fields (4 sliders,
3 constants, 24 two-element recursion arrays, a scalar recursion, the domain
counter, the delay buffer, the output `fPerm*`, and the 18 promoted
block-rate values) and reads `i0` and its input channel. faust-rs writes
the body 1,296 times and clang compiles it 1,296 times; the methods that
touch every line's fields (`instanceConstants`, `instanceClear`,
`instanceResetUserInterface`, the head of `compute`) stay unrolled per line.

Measured on prototypes (study §5), N = 36, clang `-O3 -ffast-math`, runtime
per 256-frame block at 48 kHz:

| | clang | all gates | 1 gate in 36 |
|---|---|---|---|
| `--outline` today (promotion, B) | 34 s | 6.4 ms | 0.154 ms |
| C, loop over the lines, prologue unrolled | 71 s | 5.4 ms | 0.300 ms |
| C, loop over the lines, prologue rerolled | 21 s | 5.4 ms | 0.305 ms |

C is the structural fix: the size of the generated code stops growing with
the number of instances. Two lessons from the prototypes shape this plan:
the call loop is slower than straight-line calls when few gates are on, and
thousands of unrolled stores into one large object cost clang 48 s
(`MemCpyOptPass`), so the code touching the instances outside the sample
loop must be rerolled too.

## 2. Deliverables and pass criteria

1. A FIR-to-FIR step of `fir::outline`, `share_instances`, behind
   `OutlinePolicy::share_instances` (off by default, on for the C++ CLI once
   qualified), run after outlining and promotion, with an independent
   checker reading a canonical certificate (§5) and rejecting mutations.
2. C++ emission of what it produces (an array field indexed by an
   expression, a table of control pointers, loops in the lifecycle methods):
   nothing new is expected beyond §3.5.
3. Pass criteria, on the motivating program at N = 36:
   - clang at most 25 s, faust-rs at most 12 s;
   - all gates at most 5.6 ms; one gate in 36 at most 0.16 ms (phase C1,
     unrolled calls) — the rerolled call loop (C2) only where it does not
     lose to C1 (D3);
   - outputs bit-identical to `--outline` today;
   - and generally: every hermetic corpus and impulse program emits C++ with
     the step on whenever it does without; the changed ones compute
     bit-identical outputs (`crates/compiler/tests/cpp_outline.rs`); plain
     C++, Mojo and `--outline` without the step unchanged (codegen identity
     over `tests/` and `examples/`).

## 3. Design

### 3.1 Canonical form and groups

For each function the pass created, its canonical body is built in the
pass's store: each struct field it touches replaced by a slot `S<j>`
numbered by first occurrence (with its type and access kind: scalar,
array element, read or written), each parameter renamed `P<i>` and
reordered by first use, each local renamed by first declaration. Hash-consing
then gives isomorphic functions the same `FirId`: a group is the set of
functions with the same canonical id and the same slot types and dimensions.

The parameter order is today alphabetical (`fSlowOD10` before `fSlowOD9`);
the canonical order by first use is what makes the 1,296 line bodies one id
(verified on the prototype).

### 3.2 Eligibility

A group of `n ≥ 2` functions is shared when:

- each slot names a different field in each member, and no field belongs to
  two members or to two slots (the members' state is disjoint; measured:
  53 fields per line, none shared);
- every field of a member is accessed outside the members only by
  - the lifecycle methods (`instanceConstants`, `instanceResetUserInterface`,
    `instanceClear`, `staticInit` excluded), at literal indices or in loops
    over the field's own extent,
  - `compute` outside the member calls (the output sums reading `fPerm*`,
    the resets of the promoted fields),
  - the user interface, for a control (§3.4);
- `n × dim` fits in `Int32` for every slot (`1,296 × 96,001 = 124 M` does);
- every member is called once, as the outline pass guarantees.

Anything else leaves the group as it is (refusal with its reason in the
report), so the step degrades to today's output.

### 3.3 Layout (D1)

Recommended: **structure of arrays, flattened.** Each slot `S<j>` of
dimension `d` (1 for a scalar) becomes one struct field
`fShared<g>_<j>: Array(T, n × d)`; member `k`'s element `i` is
`fShared<g>_<j>[k × d + i]`. It needs no new FIR type or node: `Array`,
`LoadTable`/`StoreTable` and integer arithmetic exist, and every backend
already prints them. The alternative, an array of structures (the
prototype's `struct Line fLines[1296]`, a line's state contiguous), needs a
FIR member access that does not exist (`FirType::Struct` is used only to
name sub-module pointers).

Memory is unchanged (the same elements, regrouped). Locality differs: the
prototype measured AoS; SoA is measured in C1 before C2 starts, and D1
revisited if it loses more than 10 % at run time.

### 3.4 Controls (D2)

Recommended: **controls stay where they are**, scalar fields with their
names, so `buildUserInterface`, `-json` `varname`, `-uim` macros and every
architecture keep seeing the same zones. A member reads its controls through
a table `fShared<g>_zones: Array(Ptr(FaustFloat), n × m)` filled once in
`instanceConstants` with the zones' addresses (`LoadVarAddress`). The read
costs one indirection, and in the motivating program controls are read only
in the block-rate part of each block (once per block per line).

The alternative moves the controls into the arrays too and lets a UI node
name an element (`&fShared0_3[17]`): it needs an index in `AddButton`,
`AddSlider` and `AddBargraph` (16 files destructure them), changes the zone
names seen by `-json` and `-uim`, and every other backend must refuse it.

The table's `n × m` address stores are one-time code; if they show the
`MemCpyOptPass` cost of §1 they are rerolled with the rest (C2), from a
static table of offsets.

### 3.5 The shared function

`<method>Shared<g>(dsp, k: Int32, P...)`: the canonical body with each slot
access turned into an access of its array at `k × d + i` (`i` the original
index, 0 for a scalar), each control read through the zone table. The
members are removed. Each call `Drop(FunCall(member, [dsp, args...]))` becomes
`Drop(FunCall(shared, [dsp, Int32(k), args in canonical order...]))`.

### 3.6 Phase C1: unrolled

Calls stay where they are, with a literal `k`: the sample loop keeps its
1,296 straight-line `if (gate) computeShared0(k, i0, input<r>)`, which keeps
B's speed with sparse gates. Every other access to a member's field becomes
the array element with a literal index `k × d + i`, folded (`fShared0_5[34]`),
or `k × d + l` in a loop over the field's extent (the clear loops of the
delay buffers).

Expected from the prototypes: the 1,296 bodies go (B + A compiled in 27 s
against B's 31 s; C1 has no 56-argument calls); what remains is the unrolled
code of §1.

### 3.7 Phase C2: rerolled

A maximal run of consecutive statements identical modulo `k`
(`k = k₀, k₀ + 1, …`) becomes a loop `for (k = k₀; k < k₀ + r; ++k)`:

- in the lifecycle methods (the constant computations, the clears, the
  control resets, the zone table) and at the head of `compute` (the resets of
  the promoted fields) — where the prototype's 48 s went;
- in the sample loop (D3), the call run
  `if (gate_k) shared(k, i0, input_{f(k)})`, when the gate and the arguments
  are affine in `k` or tabulated: a gate read from a control is
  `*fShared<g>_zones[k × m + c]`, an input channel index from a static table
  `fShared<g>_input[k]` (as the prototype's `fLineInput`).

"Identical modulo `k`": the statements' canonical forms, with every literal
array index `k × d + i` written as `(k, i)`, are one id; that is checked by
hash-consing again.

### 3.8 Interactions

- Runs after promotion: a promoted block-rate local is a member field like
  the others. Without promotion, the members' reference parameters are
  locals of the caller, per member, and a shared call would have to receive
  them: the step requires promotion and refuses otherwise.
- `-mem0` externalizes struct arrays through the memory manager: the new
  arrays follow the same path (the shared arrays are ordinary struct
  arrays). To be tested; refused until it is.
- `-uim`: unchanged under D2 (controls keep their fields).
- `-vec`, `-os`, `-ec`: the outline pass targets the scalar DSP methods; the
  step inherits that scope and is tested with each.
- Phase-2 runs (split straight-line blocks) are never isomorphic in practice
  (their spill slots differ); they are grouped like any other function if
  they are.

## 4. Exactness

A member is called once per call site, and its fields are touched only by
itself, the lifecycle methods and the listed places of `compute` (§3.2).
Replacing field `f` of member `k` by element `k × d + i` of a fresh array is
a renaming of memory cells, injective because the members' fields are
disjoint and `k × d + i` is in range. The shared body applied to `k` is the
member's body under that renaming; the control table holds the controls'
addresses, which do not change during the instance's life (set in
`instanceConstants`, before any `compute`). A rerolled loop executes the run
it replaced, in the same order.

## 5. The certificate and the checker

The step emits a certificate beside the module: for each group, the shared
function's name, `n`, the slots with their types and dimensions and array
names, and for each member `k` its original name and the field it gave each
slot; the zone table's content; the rerolled loops with their bounds.

`check_sharing(src, src_module, dst, dst_module, certificate)` shares no
code with the producer beyond the FIR matcher and builder. Rules:

1. **sections** — the struct is the source struct minus the members'
   fields plus the certificate's arrays (types and sizes `n × d`) and zone
   tables; other sections identical by id.
2. **disjointness** — every member field appears in exactly one slot of one
   member; every array index of the certificate is in range.
3. **instantiation** — for each member `k`, the shared body with `k`
   substituted, each array element `k × d + i` turned back into the member's
   field (element `i`), and each zone-table read into the control, is the
   member's source body by id (parameters back in the source order).
4. **unrolling** — every rerolled loop expanded back into its run.
5. **rewrite-back** — in every other function, each element of a
   certificate array at a literal index (or `k × d + l` in a loop over the
   field's extent) turned back into its field, and each call to a shared
   function into the call to its member `k`, gives the source function by
   id.

**Mutations.**

| mutation | defect | rule |
|---|---|---|
| `ShareWrongInstance` | a call passes `k + 1` | rewrite-back |
| `ShareOverlap` | two members given the same array element | disjointness |
| `ShareWrongStride` | element `k × d + i` written `k × (d − 1) + i` | instantiation |
| `ShareShortLoop` | a rerolled loop one iteration short | unrolling |
| `ShareStaleZone` | a zone table entry pointing at another control | instantiation |

## 6. Phases

- **C0 — census.** The grouping alone, run over the corpus and the
  motivating program, printing groups, sizes and refusals: no output
  change. Confirms §3.1 on real FIR (the study grouped C++ text).
- **C1 — shared body, unrolled** (§3.3–§3.6), its checker rules 1–3 and 5,
  three mutations; measured against B on the motivating program.
- **C2 — rerolling** (§3.7), rule 4, `ShareShortLoop`; the call-loop part
  only if D3 says so after measurement.
- **C3 — qualification and default:** `share_instances` on for the C++ CLI
  if the criteria of §2 hold; registry DIFF-CLI-018 updated.

## 7. Qualification

1. Unit tests in `crates/fir/src/outline/` on hand-built modules: a group of
   three, a refused group (shared field, a field read elsewhere), the checker
   accepting, each mutation rejected by its rule.
2. `crates/compiler/tests/cpp_outline.rs`: a fourth class with the step on
   in the numeric test (bit-identical outputs), the corpus coverage test with
   it on.
3. The motivating program measured at N = 12, 18, 36 (compile and run);
   the gated bank `gated_bank(n)` of the test file for small N.
4. Codegen identity of plain C++, Mojo, and `--outline` with the step off.
5. AGENTS.md gates; `porting/MEMOIZATION.md` for the canonical-form tables;
   `code-graphs` (new public checker items).

## 8. Decisions for the user

- **D1 — layout:** structure of arrays, flattened (recommended, no new FIR
  construct), or array of structures (needs a FIR member access)?
- **D2 — controls:** stay as named fields read through a zone table
  (recommended, no change to the UI, `-json` or `-uim`), or move into the
  arrays with indexed UI zones?
- **D3 — the call sequence:** keep it unrolled (sparse gates as fast as
  today, `compute` keeps one statement per instance), reroll it into a loop
  (smallest code, slower with sparse gates: 0.30 against 0.15 ms), or reroll
  only above an instance count?
- **D4 — default:** `share_instances` on for `--outline` once qualified, or
  a separate option?
