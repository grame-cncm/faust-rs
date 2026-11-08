# Batch form: recursion state in registers — plan (2026-11-08)

Status: implemented 2026-11-08. The plan was written before the
implementation; the outcome is in §7.

Parent: the batch contract of the Mojo backend
(`porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`). Motivation:
the GPU modal fit of `porting/faust-gpu-audio-synthesis-2026-11-07-en.md` §5.

## 1. Problem

In the batch contract, every per-voice `Struct` field lives in an arena,
element `k` of voice `v` at `arena[(b + k) * N + v]`. The generated `compute`
reads and writes those arena elements at every sample. A recursion's two-slot
buffer, for example, costs a load of `fRec0[1]`, a store of `fRec0[0]`, then
the copy `fRec0[1] = fRec0[0]`, all through device memory.

Mojo does not keep these values in registers across iterations, because the
arena pointers may alias the output buffers. On the GPU the sample loop then
waits on memory round trips. A text rewrite of the generated Mojo, which
loads every touched slot into a local before the loop and stores it back
after, made the modal fit's epoch faster at every size:

| modes | before | after | speed-up |
|---|---|---|---|
| 1,024 | 75 ms | 13.6 ms | ×5.5 |
| 4,096 | 128 ms | 64 ms | ×2.0 |
| 16,384 | 659 ms | 291 ms | ×2.3 |

This was `tests/impulse-tests/tools/batch_state_registers.py`. It is an
experiment: it matches one textual shape of one emitter's output.

## 2. The pass

`fir::batch::promote_state(src, module)` is a FIR-to-FIR pass on the
flattened module, run **before** `layout_voices`. It rewrites `compute`
only. A `Struct` field is **promoted** when all of these hold:

1. it is a scalar `DeclareVar` of a type an arena holds, or an
   `Array(T, n)` of such a type;
2. every access to it inside `compute` is one of:
   - `LoadVar` or `StoreVar` (scalar);
   - `LoadTable` or `StoreTable` whose index is an `Int32` literal (array);
3. `compute` does not take its address, use it in a `TeeVar` or shift it
   (`ShiftArrayVar`).

The pass is all or nothing per field. A field with one dynamic index (a delay
line read at `IOTA - d`) keeps all its accesses in memory. Its counter, a
scalar, is promoted.

The pass promotes nothing in a module whose `compute` contains a `Return` or
calls a function defined in the module: an early return would skip the
write-back, and a callee could touch the field behind the pass's back. This
is conservative; Faust's `compute` has neither today.

The rewrite, for every promoted element (the field itself for a scalar,
`(field, k)` for each literal `k` used):

- **entry:** a `Stack` local, `DeclareVar(local, T, Stack, init: <the original
  access>)`, placed at the head of the body block, in a deterministic order:
  field declaration order, then `k`;
- **body:** every access replaced by the local (`LoadVar` / `StoreVar` with
  `Stack` access);
- **exit:** `StoreVar` or `StoreTable(field, k, LoadVar(local))` at the end of
  the body block, **only for the elements `compute` stores into**. A
  read-only element (a control zone, a constant) is loaded once and never
  written back.

Local names are `<field>_reg` (scalar) and `<field>_reg<k>` (element);
the Mojo emitter's snake case makes `fRec0_reg1` into `rec0_reg1`. A
name already declared anywhere in the module makes the pass leave that field
alone rather than invent another name.

Why the semantics are kept:

- A voice's per-voice state is touched by its own `compute` call only; the
  layout pass refuses any other function that touches it.
- Within `compute`, every access of a promoted element is replaced
  uniformly. So the local sees the same sequence of loads and stores as the
  field did, and holds the field's final value at exit.
- No other path reaches the field: no address, no callee, no early exit.
- The output buffers are distinct from the state in the Faust contract, so
  nothing reached through them can change a promoted value.

`layout_voices` then sees the entry loads and exit stores only. It lays them
out unchanged, and its checker validates the composed output as before. The
pass is a generic scalar replacement and does not depend on the batch layout.
It is used only for the batch form, because the scalar backends' compilers
already keep these values in registers.

## 3. The independent checker

`fir::batch::promote_check::check_promotion(src, src_module, dst, dst_module)`
shares no code with the producer beyond the FIR matcher. It restates the
naming convention on its own. Both modules are imported into one checking
store, where identical trees intern to one id. The rules, in the order they
are checked:

1. **sections.** Everything but the `compute` body is unchanged, compared by
   id: the module header, the structure, globals, static declarations,
   sub-modules, every other function, and `compute`'s signature.
2. **entry-exit.** The output body is `entry ++ middle ++ exit`:
   - `entry` is the run of `Stack` declarations named `<field>_reg`
     (initialised from the scalar field) or `<field>_reg<k>` (from element
     `k`, an `Int32` literal), each declared once. A declaration named for
     one element of a field but initialised from another is a violation;
   - `exit` is the run of `Struct` stores of an entry local into the element
     it was read from, each at most once.
3. **early-exit.** When anything is promoted, the source `compute` has no
   `Return` and calls no function defined in the module.
4. **literal-only.** In the source `compute`, a field with a promoted element
   is accessed only as a scalar or at literal indices: no dynamic index, no
   address, no `TeeVar`, no shift.
5. **memory-access.** `middle` reaches no promoted field through `Struct`
   memory. This is the defect that loses writes, so it is reported by name
   before the next rule.
6. **unpromote.** Replacing in `middle` every load of an entry local by the
   access it was initialised from, and every store to it by a store to that
   element, gives the source body exactly (by id).
7. **write-back.** An element is in `exit` exactly when the source `compute`
   stores into it: no lost write, no spurious one.

`promote_state` returns only an output its checker accepts, and fails closed
otherwise, as `layout_voices` does.

**Rejecting mutations.** The producer has a `Mutation` switch (tests only).
Each mutation must be rejected by a named rule:

| mutation | defect | rule |
|---|---|---|
| `DropWriteBack` | one written element not stored at exit | write-back |
| `SpuriousWriteBack` | one read-only element stored at exit | write-back |
| `LeaveOneAccess` | one body access left on the field | memory-access |
| `WrongElement` | an entry initialiser reads `k + 1` | entry-exit |
| `PromoteDynamic` | a field with a dynamic index promoted anyway | literal-only |
| `ExitBeforeBody` | the write-backs placed before the body | memory-access |

## 4. Integration

- `MojoOptions::batch_registers: bool`, default `true`, applies the pass
  before `layout_voices` when `batch` is set. The `mojo_batch` example gains
  `--no-registers` to emit the old form, for A/B measurements.
- No other backend and no non-batch output changes. The interp batch path
  (`generate_interp_batch_module`) is not wired to the pass; it is used to
  check it (§5).

## 5. Qualification (pass criteria)

1. Unit tests in `crates/fir/src/batch/`:
   - the pass on the layout tests' hand-built module (scalars, recursion
     buffers of two element types, a delay line with its counter, a control,
     shared generator state): which fields are promoted, which are written
     back, and the delay line left alone;
   - the checker accepting the output;
   - every mutation of §3 rejected by its rule;
   - the refusals (`Return`, a module call) leaving the module unchanged.
2. A structural non-regression test: the promoted `compute` of the modal
   voice has no `Struct` access inside its sample loop.
3. Numeric: `crates/compiler/tests/batch_voices_interp.rs` runs every program
   also through `promote_state` then `layout_voices`. Every voice must be
   **bit-identical** to its scalar instance, at `opt_level` 0 and max, for 1,
   3 and 64 voices. The corpus GPU voices are included
   (`gpu_modal_fad_voice` added).
4. Codegen identity: the non-batch Mojo output, and every other backend's
   output, unchanged. The batch output changes as intended.
5. On the GPU (opt-in, needs Mojo):
   - `make mojo-gpu-batch`: the impulse lane with the batch contract passes;
   - the harnesses' `validate` modes pass (choir, bells, modal fit);
   - the modal bench with the generated code matches the script's timings
     within run-to-run noise.

   Then the script and its build in the Makefile are removed.
6. The AGENTS.md gates, including `compile-budget-check` (`fir` and `codegen`
   are touched), and `code-graphs` regenerated for the new public items.

## 6. Public API

New public items, Rust-only (`adapted`, no C++ counterpart):

- `fir::batch::promote_state`;
- `fir::batch::PromotedState` and `fir::batch::PromotedElement` (the
  report: each promoted element, whether written back, and the fields kept
  in memory with the reason);
- `fir::batch::PromoteError`;
- `fir::batch::promote_check::{check_promotion, PromotionViolation}`;
- `MojoOptions::batch_registers`.

The compatibility registry is not affected: the batch contract is Rust-only
and has no C++ counterpart.

## 7. Outcome (2026-11-08)

Implemented as planned.

- `crates/fir/src/batch/promote.rs` is the producer.
- `crates/fir/src/batch/promote_check.rs` is the checker. Its rules are those
  of §3 in that order.
- `MojoOptions::batch_registers` turns the pass on, and
  `mojo_batch --no-registers` turns it off.

Two corrections to the plan came from the tests:

- **`PromoteDynamic` was first a no-op.** The test module's delay line has no
  literal access, so "promote its literal elements" promoted nothing, and the
  mutation test failed: it was accepted. The mutation now also promotes
  element 0. The test caught the vacuous mutation, which is its purpose.
- **Rule mapping.** `ExitBeforeBody` and `LeaveOneAccess` are rejected by
  **memory-access** (the stores and the load left in `middle` reach a
  promoted field), and `WrongElement` by **entry-exit** (the local is named
  for one element and initialised from another). The table of §3 is updated.

Qualification (§5), all passing:

1. **Unit tests.** Seven tests in `crates/fir/src/batch/promote_tests.rs`.
   On the hand-built module:
   - `fHslider0` (read only), `fRec0[0..1]`, `iRec1[0..1]` and `fIOTA` are
     promoted;
   - `fRec0[0]`, `iRec1[0]` and `fIOTA` are written back;
   - `fVec0` stays in memory ("a dynamic index");
   - an address keeps its field in memory;
   - a `Return` or a call to `instanceClear` promotes nothing;
   - the six mutations are rejected, each by its rule.
2. **Structural.** In `crates/compiler/tests/batch_voices_interp.rs`, the
   modal voice's sample loop reaches the arenas without the pass and reaches
   none with it. Nothing of that voice is kept in memory.
3. **Numeric.** The same file runs the eight inline programs and the five
   corpus GPU voices both ways. Every voice is bit-identical to its scalar
   instance, at opt 0 and max, for 1, 3 and 64 voices. The programs cover a
   variable delay (a promoted counter beside a delay line kept in memory), an
   integer recursion, a generated table, a button and the sample rate.
4. **Codegen identity.** Non-batch output is unchanged: the pass runs only
   under `batch && batch_registers`.
   - `-lang mojo` (single and double) over every `.dsp` of `tests/` and
     `examples/`: 1,047 outputs identical to the base build, 241 failing on
     both sides;
   - `mojo_batch --no-registers` against the base batch output: 19 of 19
     identical.
5. **GPU** (Apple M5, Mojo 1.3 nightly):
   - `make mojo-gpu-batch`: 131/131, the two known failures (`sound`,
     `math`) excluded as before;
   - `validate`: the modal fit (worst 1.07e-3, as before), the choir (8.7e-5)
     and the carillon (1.5e-4);
   - the modal epoch equals the text rewrite's: 14.8 / 67.8 / 292 ms against
     14.7 / 67.8 / 293 ms at 1,024 / 4,096 / 16,384 modes, against 75 / 128 /
     659 ms with the state in the arenas;
   - the carillon at 256 bells: 0.46 ms per block, against 0.84 ms recorded
     before the pass. That is ×2.5 over the CPU threads measured in the same
     process.

   The text rewrite (`tests/impulse-tests/tools/batch_state_registers.py`)
   is removed.
6. **Gates.** fmt, clippy, the workspace tests, the five structure checks
   and `golden-check` pass. `compile-budget-check` cannot run on this machine
   (calibration under its floor). A manual A/B of `-lang cpp` over the impulse
   programs shows no difference, as expected for a pass that runs only in
   batch mode.

What the pass does not do:

- A delay line at a dynamic index stays in arena memory. A circular buffer
  cannot be held in registers, and on the GPU it is the remaining state
  traffic of programs with delays.
- Promoted locals are not capped. A program with hundreds of literal state
  elements would ask for as many registers. The GPU then spills to private
  memory, which costs about what the arena access did. No program measured
  hit this.
- The non-batch Mojo backend is not changed. Whether the scalar Mojo `compute`
  has the same problem on the CPU was not measured.
