# faust-rs structure and readability plan

**Date:** 2026-08-18
**Status:** Phase 0 — audit and plan only. No code has been changed.
**Scope:** the whole workspace (31 crates, 272 411 lines of Rust).
**French twin:** [`faust-rs-structure-readability-plan-2026-08-18-fr.md`](faust-rs-structure-readability-plan-2026-08-18-fr.md) (EN is canonical).
**Companion docs:** [`delay-rs-simplification-experiment-2026-06-21-en.md`](delay-rs-simplification-experiment-2026-06-21-en.md),
[`signal-prepare-simplification-experiment-2026-06-22-en.md`](signal-prepare-simplification-experiment-2026-06-22-en.md),
[`faust-rust-porting-plan-en.md`](faust-rust-porting-plan-en.md).

**Goal:** make the compiler readable, documentable, and modifiable in independent
pieces, while emitting byte-identical FIR. This is organisation work — file
boundaries, naming, module headers — not rewriting, optimising, or bug fixing.

---

## 0. How to reproduce every number in this document

All metrics come from one script, kept with this plan:
`porting/scripts/structure-metrics.py`. It is a brace scanner that blanks out
comments and string literals before counting; it is deliberately syntactic, and
it undercounts rather than guesses.

```bash
python3 porting/scripts/structure-metrics.py > /tmp/metrics.json
```

Per-section commands are given inline below. Nothing in this document is an
estimate.

---

## 1. The compilation chain as it actually is

### 1.1 Stage crates

| Crate | Role | Public entry point | Lines (prod / test) |
|---|---|---|---|
| `parser` | Faust source → box AST (`lrpar`/`lrlex`) | `parse_program`, `parse_file*` | 6 011 / 1 577 |
| `boxes` | Box construction and matching over `tlib::TreeArena` | `BoxBuilder`, `match_box` | 3 804 / 773 |
| `eval` | Box evaluator (pipeline phase 4), pattern matcher merged in | `eval_entrypoint*`, `eval_process*` | 8 670 / 2 177 |
| `propagate` | Box → signal propagation, forward/reverse AD | `propagate_typed*` | 10 878 / 2 571 |
| `signals` | Signal construction and matching | `SigBuilder`, `match_sig` | 3 250 / 699 |
| `normalize` | Signal normalisation and algebraic simplification | `normalize_signal` | 4 546 / 0 (inline only) |
| `sigtype` | Signal type system, interval-carrying lattice | `infer` | 4 150 / 0 (inline only) |
| `transform` | Mid-level lowering: `signal_prepare` → `signal_fir` → vector | `compile_signals_to_fir_fastlane_*` (5 variants) | 47 069 / 15 333 |
| `fir` | FIR store, matcher, checker, inliner | `FirBuilder`, `match_fir`, `check_fir` | 10 493 / 4 071 |
| `codegen` | Backend emission from FIR (17 backend directories) | per-backend `emit_*` | 45 933 / 8 930 |
| `compiler` | Top-level facade: library API + `faust-rs` CLI | `compile_source_to_*`, `main.rs` | 16 011 / **21 449** |

Supporting crates: `tlib` (tree arena foundation), `interval`, `ui`,
`diagnostics`, `draw`, `foreign-call`, `xtask`, plus eight `*-ffi` crates.

Reproduce:
```bash
for d in crates/*/; do find "$d" -name '*.rs' | xargs wc -l | tail -1; done
```

### 1.2 Dependency shape

The generated graph (`docs/code-graphs/internal-crate-deps.dot`, 102 edges) shows
a clean layering: `compiler` depends on 14 crates, `codegen` only on `fir` and
`foreign-call`, and the FFI adapters sit strictly downstream. No cycle exists.
**The crate-level architecture is not the problem** — this audit found no
dependency-direction defect. The problems are inside crates and at their entry
points.

### 1.3 Discrepancies with `AGENTS.md` §2

Three integrations are asserted there. Checked one by one:

- **`patternmatcher` merged into `eval`** — confirmed:
  `crates/eval/src/pattern_matcher.rs` exists and is used from `apply.rs`.
- **`parallelize` integrated into `transform`** — confirmed, as
  `crates/transform/src/schedule/` (breadth-first, depth-first,
  reverse-breadth-first strategies).
- **`extended` math nodes integrated into `signals`** — **partially misleading.**
  The math node *constructors* are in `signals` (`SigBuilder::acos`,
  `::atan2`, …), but the string "extended" survives only in
  `crates/boxes/src/print.rs` and in the interpreter's opcode tables. The claim
  is true in substance and stale in vocabulary; a reader grepping for
  `extended` lands in the wrong crate.

One further mismatch, not a discrepancy but worth recording: **three workspace
members are declared placeholders** — `graph` (21 lines, "Graph algorithms crate
placeholder"), `doc` (22 lines, "placeholder"), `algebra` (29 lines). They are
real Cargo members carrying no implementation, so every crate-level view of the
workspace overstates its structure by three.

---

## 2. Measured metrics

### 2.1 Volume

272 411 lines of Rust across 31 crates: **212 571 production, 59 840 in
`tests/` or `tests.rs` files**, plus a further ~28 800 in `#[cfg(test)]` modules
inside production files — **32 % of the Rust is test code**, which is
normal-to-low for a compiler and is not a restructuring target.

Two crates hold 44 % of the production code: `transform` (47 069) and `codegen`
(45 933). `compiler` is third at 16 011 — but it carries **21 449 lines of test
code, more test than production**, and 652 of the workspace's ~2 500 `#[test]`
functions. A quarter of the suite sits at the top of the pipeline, where a
failure identifies the whole compiler rather than a stage. That is an altitude
problem to record, not a phase in this plan.

`normalize` and `sigtype` have no `tests/` directory at all; their tests are
inline `#[cfg(test)]` modules only.

### 2.2 Files over 1 500 lines: 31 production files

| Lines | File |
|---|---|
| 3 336 | `crates/fir/src/checker.rs` |
| 3 278 | `crates/codegen/src/backends/rust/mod.rs` |
| 3 186 | `crates/codegen/src/backends/cpp/mod.rs` |
| 3 150 | `crates/parser/src/lib.rs` |
| 3 126 | `crates/codegen/src/backends/wasm/mod.rs` |
| 2 905 | `crates/fir/src/inliner.rs` |
| 2 855 | `crates/box-ffi/src/lib.rs` |
| 2 665 | `crates/cranelift-ffi/src/factory.rs` |
| 2 610 | `crates/sigtype/src/rules.rs` |
| 2 493 | `crates/codegen/src/backends/c/mod.rs` |
| 2 419 | `crates/eval/src/lib.rs` |
| 2 339 | `crates/transform/src/signal_fir/vector/lower/signal.rs` |
| 2 304 | `crates/compiler/src/lib.rs` |

`structure-check`'s `MAX_PRODUCTION_LINES` is 2 400 and applies only to
`transform` and `compiler`. Eleven files above it sit in crates the gate does not
watch.

### 2.3 Functions over 150 lines: 116

| Lines | Function |
|---|---|
| 1 640 | `try_execute_block_io_inner` — `codegen/src/backends/interp/executor.rs:527` |
| 1 449 | `compile_instr` — `codegen/src/backends/interp/fbc_to_cpp.rs:490` |
| 822 | `artifacts_to_json` — `wasm-ffi/src/lib.rs:1080` |
| 773 | `propagate_inner` — `propagate/src/engine.rs:132` |
| 767 | `build_module` — `transform/src/signal_fir/module/build.rs:541` |
| 727 | `run_source_mode` — `compiler/src/cli/source_mode.rs:27` |
| 676 | `match_fir` — `fir/src/matcher.rs:314` |
| 643 | `verify_vector_plan` — `transform/…/vector/verify/check.rs:49` |

### 2.4 `impl` blocks over 500 lines: 24

Largest: `fir/src/checker.rs:582` (2 649 lines),
`codegen/…/interp/compiler.rs:218` (1 891),
`transform/…/vector/lower/signal.rs:461` (1 740),
`codegen/…/interp/executor.rs:443` (1 725),
`codegen/…/cranelift/lowering.rs:211` (1 641).

### 2.5 Module tree depth

Maximum depth below `src/` is **3**, reached only in `codegen` and `transform`.
The tree is flat; depth is not a readability problem here.

### 2.6 Documentation coverage

- **Modules without a `//!` header: 7 out of ~700.** The codebase is
  well documented at module level; this is *not* a lever.
- **Public items without rustdoc: 251**, concentrated in `codegen` (67),
  `transform` (57), `interval` (20), `interp-ffi` (20), `draw` (19).
  `transform` is already gated by `cargo rustdoc -p transform --lib -- -D
  missing-docs`; the other 30 crates have no floor.

### 2.7 Anonymous tuple returns with ≥3 fields: 13

Ten of the thirteen are test fixtures. Only three sit on production paths
(`ui/src/lib.rs:1096`, `fir/src/inliner.rs:2126`, `eval/src/environment.rs:505`).
**This is not a lever either** — the prompt's suspicion is not borne out.

### 2.8 The finding that dominates: parameter accretion

**240 production functions take more than 6 parameters.** The extremes:

| Params | Function |
|---|---|
| 22 | `new` — `codegen/src/backends/interp/factory.rs:86` |
| 16 | `scaffold_function_body` — `codegen/src/backends/wasm/mod.rs:789` |
| 16 | `declare_jit_function` — `codegen/src/backends/cranelift/jit_data.rs:255` |
| 15 | `lower_signals_to_fir` — `compiler/src/signal_lowering.rs:605` |
| 11 | `compile_fastlane_inner` — `transform/src/signal_fir/mod.rs:664` |

The same pressure surfaces at every stage boundary as **telescoping entry
points** — one function per combination of optional arguments, with the
combination spelled out in the name:

| Stage | Variants | Names |
|---|---|---|
| `parser` | 6 | `parse_program`, `…_with_metadata`, `…_with_precision_and_metadata`, `…_with_imports_and_metadata`, `…_with_imports_and_precision_and_metadata`, `…_with_remote_imports_and_precision_and_metadata` |
| `parser` | 5 | `parse_file_with_imports*` family |
| `eval` | 5 | `eval_entrypoint`, `…_with_source_context`, `…_with_stats`, `…_with_stats_and_source_context`, `…_with_source_context_and_cancel` |
| `eval` | 4 | `eval_process*` family |
| `transform` | 5 | `compile_signals_to_fir_fastlane_with_ui`, `…_with_ui_and_shadow`, `…_clocked`, `…_clocked_with_timing`, `…_clocked_with_timing_and_origins` |
| `propagate` | 3 | `propagate_typed`, `…_with_ui`, `…_with_ui_options` |

In `transform` all five delegate to one `compile_fastlane_inner` with 11
positional parameters, three of which are `Option<…>` passed as `None` by the
short variants.

Reproduce §2.3, §2.4, §2.8:
```bash
python3 porting/scripts/structure-metrics.py | python3 -m json.tool | less
```

### 2.9 Cross-backend duplication in `codegen` — smaller than assumed

Pairwise similarity of the seven textual emitters' `mod.rs`, on normalised
non-comment lines (`difflib.SequenceMatcher.quick_ratio`):

| | asc | c | cmajor | codebox | cpp | julia | rust |
|---|---|---|---|---|---|---|---|
| **asc** | — | 37 % | 40 % | 33 % | 38 % | 44 % | 38 % |
| **c** | | — | 29 % | 24 % | **57 %** | 41 % | 37 % |
| **cpp** | | | | | — | 33 % | 31 % |
| **rust** | | | | | | | — |

Of 3 382 significant lines (≥40 chars) across the seven, 838 occurrences (279
distinct lines) appear in two or more backends — and the most-shared lines are
boilerplate: the per-backend `CodegenError` type with its `new`/`Display`/`Error`
impls, and `decode_module`, each duplicated across all seven.

`c_family.rs` (1 291 lines) already factors the c/cpp pair (20 and 22
references); `cmajor`, `codebox` and `rust` reference it zero times.

**This contradicts the premise that `codegen` is where restructuring reduces
volume.** Its 46 869 production lines are dominated by `interp` (12 848) and
`cranelift` (4 620) — two genuinely large machines — not by twin text emitters,
which are 21–57 % alike and 7–34 % inline test code. The extractable duplication
is real but measured in hundreds of lines, not thousands.

---

## 3. Readability diagnosis, ordered by cost to the reader

**D1 — Optional context is encoded in names and positions, not in types.**
240 functions above 6 parameters, six telescoping entry-point families across
five crates. To call a stage you must know which of five near-identical names
carries the argument you need, and to read one you must count positional
`None`s. This is the defect with the widest blast radius: it sits exactly at the
boundaries a reader uses to orient. It is also the cheapest to fix, because the
remedy — one options struct per stage boundary — is pure delegation.

**D2 — Two interpreter functions are longer than most crates.**
`try_execute_block_io_inner` (1 640 lines) and `compile_instr` (1 449) hold the
entire FBC execution and C-emission dispatch in one body each. No amount of
module documentation makes a 1 640-line function readable; it must be split by
opcode family. Together with `executor.rs`, `compiler.rs` and `fbc_to_cpp.rs`
(2 170 / 2 225 / 2 255 lines) this is the densest unreadable region in the
workspace.

**D3 — `fir/checker.rs` is a 2 649-line `impl` inside a 3 336-line file.**
It is the FIR validity authority, consumed by every backend. Its size is a
review hazard precisely because it is the thing that decides whether everything
else is correct.

**D4 — `structure-check` watches two crates out of 31.**
Its 2 400-line threshold applies to `transform` and `compiler` only. Eleven
production files above that threshold live in `fir`, `codegen`, `parser`,
`sigtype`, `box-ffi` and `cranelift-ffi`, where nothing measures them. The gate
does not lie, but it is read as if it covered the workspace.

**D5 — 251 public items carry no rustdoc**, and only `transform` has a
`missing_docs` floor. A reader who reaches for the API index gets names without
contracts.

**D6 — Three placeholder crates inflate every structural view.**
`graph`, `doc`, `algebra`: 72 lines between them, three of 31 workspace members.

**Explicitly not problems** — measured and dismissed, so no phase spends effort
there: module tree depth (max 3), missing module headers (7 files), anonymous
tuple returns (13, mostly fixtures), crate-level dependency direction (clean, no
cycles), and the volume of test code (32 %, appropriate).

---

## 4. What "restructured" will mean

The plan commits to these, and each phase states which it advances:

1. **One file names one step.** Every production file must be describable in one
   sentence naming its pipeline stage. Where that is impossible today, the file
   is split, not annotated.
2. **Optional context is a named type.** No stage boundary gains a new `_with_x`
   variant; optional inputs become fields of a documented options struct with a
   `Default`. Existing telescoping families collapse.
3. **Every module keeps its `//!` header**, answering: what it does, what goes in
   and out, what it guarantees, and which C++ source it mirrors
   (`master-dev-ocpp-od-fir-2-FIR19` / `8eebea429`).
4. **Producer/checker separation is preserved and extended, never weakened.**
   The `structure-check` rules that ban a `check.rs` from importing its producer
   stay; any new split respects them.
5. **Reading one step must not require reading its neighbours.**
6. **Static dispatch only.** Enums and monomorphisation; no `dyn` introduced to
   factor code.

---

## 5. Phases

Each phase is independent and severable: abandoning phase N+1 leaves the
repository coherent. Each lands as *move first, edit second* — never one commit
mixing file motion with content change.

### P1 — Collapse the `transform` entry-point family (first phase, recommended)

**Target:** the five `compile_signals_to_fir_fastlane_*` in
`crates/transform/src/signal_fir/mod.rs:508-663` and their 11-parameter
`compile_fastlane_inner`.
**Transformation:** one entry point taking `&SignalFirRequest` — a documented
struct whose fields are today's positional arguments, with `clock_domains`,
`timing_sink` and `signal_origins` as `Option` fields and a `Default`. Migrate
the in-workspace call sites (`compiler/src/signal_lowering.rs`, tests).
**Neutrality proof:** pure delegation, no logic touched. Golden FIR diff over
the impulse corpus + full test suite. The `docs/code-graphs/public-api-baseline.txt`
diff is *expected here* and is the reviewable record of the boundary change.
**Pass criteria:** all gates green; baseline diff shows exactly the removed
variants and the new entry; `signal_fir/mod.rs` loses ≥100 lines.
**Commit size:** one commit.

### P2 — Same treatment, one crate per commit: `parser`, `eval`, `propagate`

Four more families (6 + 5 + 5 + 4 + 3 variants). Identical shape to P1, done
only after P1 has proven the method end to end. One commit per crate, in
pipeline order.

### P3 — Split the interpreter backend

**Target:** `codegen/src/backends/interp/{executor,compiler,fbc_to_cpp}.rs`
(6 650 lines, containing the 1 640- and 1 449-line functions).
**Transformation:** split each mega-function by opcode family into sibling
modules under `interp/`, mirroring the existing `vector/` sub-module layout that
R3 produced in `transform` — the precedent to imitate.
**Neutrality proof:** byte-identical FIR is not enough here (the interpreter
*executes*); requires the impulse oracle including the `opt_level=0` vs `max`
parity check AGENTS.md §5 mandates.
**Pass criteria:** no production file above 1 500 lines under `interp/`; no
function above 400.

### P4 — Extract the duplicated backend boilerplate

**Target:** the `CodegenError` type with its `new`/`Display`/`Error` impls and
`decode_module`, duplicated verbatim across all seven textual backends.
**Transformation:** move into `backends/mod.rs` or extend `c_family.rs`'s role.
**Neutrality proof:** per backend, byte-identical emitted output on the golden
corpus — established backend by backend before the abstraction lands, per the
prompt's prohibition.
**Expected size:** a few hundred lines removed, not thousands. Worth doing, not
worth leading with.

### P5 — Split `fir/checker.rs`

The 2 649-line `impl` broken along the rule families it checks, preserving
checker independence. Deferred behind P3 because it is the higher-risk artefact:
everything downstream trusts it.

### P6 — Extend the structural floor

Bring `structure-check`'s threshold to bear on the whole workspace rather than
two crates, and lower `MAX_PRODUCTION_LINES` as P3/P5 make it achievable. Add a
`missing_docs` floor per crate as each is cleaned. **This phase records the
result of the others**; it cannot run first.

### P7 — Decide the fate of the placeholder crates

`graph`, `doc`, `algebra`: implement, fold into their consumers, or remove from
the workspace. A decision, not a refactor — needs the maintainer's intent.

---

## 6. Recommended order, and where it contradicts the brief

The brief proposed `transform` → `codegen` → `compiler` by size, with `codegen`
as "the only place restructuring reduces volume". **The measurements contradict
that on two points**, so the recommended order differs:

- `transform`'s `signal_fir/vector/**` is *already* restructured — 13 coherent
  sub-modules of 2 000–3 500 lines each, the output of the R3 work. Attacking
  `transform` as a bulk target would re-do finished work. Its remaining defects
  are one oversized file (`lower/signal.rs`) and its entry points (P1).
- `codegen`'s twin emitters are 21–57 % alike, not near-duplicates, and 7–34 %
  of each is inline test code. The volume is in `interp` and `cranelift`, which
  are large because they are machines, not because they are duplicated. The
  extractable duplication is hundreds of lines (P4), not thousands.

Recommended order: **P1 → P2 → P3 → P4 → P5 → P6**, with P7 raised whenever the
maintainer wants to decide it. The logic is smallest-provable-instance first: P1
is one file, one mechanical transformation, and it exercises every gate including
the public-API baseline added on 2026-08-18 — if the method is wrong, P1 is where
that shows up cheaply.

---

## 7. What this plan will not do

- **Touch numerically sensitive lowering without an oracle.** Vector WASM has no
  C++ reference (`G5-W5`/`G5-W6`); any phase reaching it relies on repository
  tests alone and must say so rather than implying golden coverage.
- **Re-shape code that deliberately mirrors C++ structure** for parity review.
  Where a Rust file tracks a C++ file function-for-function, that alignment is an
  asset; splitting it costs more than it returns.
- **Merge stages that merely resemble each other.** Two backends emitting similar
  text are not proven to have the same role.
- **Fix bugs found along the way.** They get reported, not folded into a
  restructuring commit.
- **Reduce the test suite.** At 32 % it is proportionate; the tests are the
  parity evidence that makes this work safe.
- **Touch the `xtask`, `cranelift-ffi` or `*-ffi` crates under a golden-FIR
  claim.** They emit no FIR; their only neutrality evidence is the test suite,
  and no phase may pretend otherwise.
