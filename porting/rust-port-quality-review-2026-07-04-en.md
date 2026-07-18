# faust-rs port quality review — code, documentation, improvement opportunities

**Date**: 2026-07-04 (at commit `e13c4e4a`, branch `main-dev`)
**Scope**: whole workspace — 30 crates, ~141 000 lines of Rust under `crates/*/src`
**Method**: static metrics (LOC, lint posture, unsafe/panic/doc density), full
workspace build + clippy + test run, structural reading of the pipeline crates,
inventory of the testing and documentation infrastructure.

---

## 1. Executive summary

The port is in **very good health for a compiler of this size**. The whole
workspace builds clean, `cargo clippy --workspace -- -D warnings` passes with
zero findings, the full test suite (~1 445 `#[test]` functions) passes with
zero failures, and CI enforces format + clippy + tests + golden-output checks
on three OSes. The architecture is genuinely Rust-idiomatic rather than a
transliteration: arena/`TreeId` hash-consing instead of pointer trees,
side-table memoization instead of per-node property lists, typed error enums
instead of exceptions, and `unsafe` statically forbidden outside the FFI
boundary crates. Documentation is unusually rich for a port (73 source files
carry explicit C++ provenance sections; 111 design documents plus a 100-day
split journal live in `porting/`).

The main quality debts are: (a) the known **propagation performance gap**
(separate plan, see
[cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md](cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md));
(b) **changelog-style module headers** in the newest code (`signal_fir`) that
document the development history instead of the current design; (c) a dozen
**scaffold crates/backends** (17–30 lines) whose placeholder status is honest
but inflates the apparent surface; (d) **near-parallel textual backends**
(c/cpp/julia ≈ 1 700 LOC each, 42–44 functions each) with no shared emitter
core; and (e) no index over the 111 `porting/` documents.

---

## 2. Architecture and workspace layout

### 2.1 Layering — good

The crate graph mirrors the C++ pipeline stages while keeping boundaries the
C++ code never had:

- **Foundation**: `tlib` (1.9 k — arena, hash-consing, De Bruijn helpers),
  `errors`, `utils`, `interval` (3.4 k, fully ported, 65 tests).
- **Front end**: `parser` (3.1 k, grammar via `cfgrammar`/`lrpar`), `boxes`
  (2.6 k), `eval` (8.2 k), `propagate` (9.1 k), `signals`, `ui`.
- **Middle end**: `normalize` (4.4 k), `sigtype` (3.3 k), `transform` (16.9 k
  — `signal_prepare` + `signal_fir` lowering, FAD/RAD), `fir` (11.4 k — IR,
  checker, inliner).
- **Back end**: `codegen` (36.8 k) — implemented: interp (16.3 k), cranelift
  (5.8 k), wasm (5.5 k), cpp/c/julia (~1.7 k each), asc (1.5 k).
- **Distribution/FFI**: `box-ffi`, `signal-ffi`, `interp-ffi`, `cranelift-ffi`,
  `tree-ffi`, `wasm-ffi`, unified under `faust-ffi` (canonical `libfaust`
  staticlib/cdylib). This isolation is a real improvement over the C++
  monolith: the entire core is `#![forbid(unsafe_code)]` at the workspace
  level, and only the FFI crates opt out explicitly in their own `Cargo.toml`.
- **Tooling**: `xtask` (5.9 k) with 20+ subcommands (golden checks, parity
  reports, interp trace tooling, backend alignment, code graphs).

### 2.2 Scaffolds — acceptable but should be flagged more visibly

`algebra` (28 LOC), `graph` (20), `doc` (21), and ten backend placeholder
modules (`llvm`, `cmajor`, `jax`, `vhdl`, `sdf3`, `csharp`, `dlang`, `rust`,
`codebox`, `jsfx` — 17 lines each) are pure scaffolds exposing only a
`crate_id()`/`backend_id()`. Their doc headers honestly say "scaffold only",
which is the right convention, but nothing at the workspace level distinguishes
implemented from planned surface (see §5.3).

---

## 3. Code quality

### 3.1 Static discipline — excellent

- `[workspace.lints.rust] unsafe_code = "forbid"` with per-crate FFI opt-outs;
  measured `unsafe` concentration confirms the policy works: cranelift-ffi 301,
  box-ffi 161, interp-ffi 159, signal-ffi 153 occurrences vs essentially zero
  in the pipeline crates.
- `cargo clippy --workspace -- -D warnings`: **clean** (verified this review).
- `cargo fmt --check`, clippy `-D warnings`, and `cargo test --workspace
  --all-targets` run in CI on ubuntu/macos/windows, plus a golden-output check
  and a C++ backend diff report. Very few projects gate this hard.
- Only 41 `#[allow(...)]` attributes across 141 k lines, and zero
  `TODO`/`FIXME`/`HACK` markers — deferred work is tracked in `porting/`
  documents instead of code comments, which is a deliberate and consistent
  convention.

### 3.2 Panic/expect discipline — good, with one audit-worthy pocket

277 `panic!` calls sound alarming but decompose almost entirely into test code
(transform: 194 total, of which 152 in `signal_fir/tests.rs` and 41 in
`signal_prepare` test contexts) and `unreachable!`-style decode guards after a
match on an already-validated tag (e.g. `propagate/engine.rs`'s
"flat X node must decode to BoxMatch::X"). These encode real invariants of the
flat-box validation boundary and panicking there is defensible.

`.expect()` outside test code is rarer: propagate 54, codegen 35, eval 17,
fir 11, transform 8, compiler 4. The propagate count is the one worth an
audit pass: several are contract expectations against tables built in a
*different* phase (e.g. `control_ids` registered during UI extraction) — those
are cross-phase invariants where a typed `PropagateError` variant would turn a
future desynchronization bug from a panic into a diagnosable error.

### 3.3 Idiomatic quality — strong

The port consistently chooses Rust-native designs over transliteration, and
(as the 2026-07-03 upstream commits showed) C++ is now converging on several
of them:

- `TreeArena` + `TreeId(u32)` hash-consing with per-arity interner maps,
  instead of intrusive pointer-chained global tables.
- Side-table `AHashMap` memoization keyed by plain ids (`EvalCacheKey`,
  `PropagateMemo`, `ArityCache`) instead of per-node property lists.
- Typed error enums per crate with `Display`/diagnostic impls (`PropagateError`
  with node ids and arity payloads, `EvalError` with 19 variants), `Result`
  threading instead of `faustexception`.
- Cooperative cancellation (`Arc<AtomicBool>` in `LoopDetector`) instead of
  `process::exit` — libfaust-host-safe by construction.
- Newtype ids (`FlatBoxId`, `SigId`, `SymId` as interned `u32`) enforcing
  phase boundaries in the type system, something the C++ `Tree`-everywhere
  design cannot express.

### 3.4 Duplication and size hot spots — the main code-level debt

- **Textual backends are near-parallel implementations**: `cpp` (1 756 LOC,
  44 fns), `c` (1 676, 42), `julia` (1 760, 44) track each other function for
  function. The C++ compiler has the same shape (one `*_code_container` per
  language), so this is faithful — but Rust could do better with a shared
  C-family emitter parameterized over syntax (type names, method syntax,
  array indexing), which would cut ~2×1 700 lines and make backend drift
  (fixing a bug in cpp but not c) structurally impossible. The
  `backend-align-smoke` xtask exists precisely because this drift risk is
  real.
- **The four AD engines in `propagate`** (`forward_ad` 1 765, `reverse_ad`
  1 265, `transpose_ad` 1 076, `stateful_rad` 1 070) share traversal
  scaffolding worth factoring once YOLO-RAD (see
  [yolo-linearize-once-rad-analysis-2026-05-21-en.md](yolo-linearize-once-rad-analysis-2026-05-21-en.md))
  settles the architecture — premature now, worth revisiting after.
- **God files**: `signal_fir/tests.rs` (4 207), `box-ffi/lib.rs` (2 850),
  `fir/checker.rs` (2 772), `wasm/mod.rs` (2 749), `fir/inliner.rs` (2 575),
  `eval/lib.rs` (2 403). The project has a good track record of splitting
  these (boxes, eval, propagate, signal_prepare were all split in recorded
  refactors); the same treatment is due for `fir` and the wasm backend.

---

## 4. Testing and validation — the port's strongest asset

- ~1 445 `#[test]` functions; full workspace suite passes (verified this
  review, 0 failures, 3 ignored).
- **Differential testing against C++ at four levels**, which is the right
  methodology for a port:
  1. `tests/golden/{rust,cpp}` + `xtask golden-check[-cpp]` — generated-code
     identity;
  2. `tests/impulse-tests` + `crates/impulse-runner` — a genuine port of the
     C++ 4-pass oracle harness (baselines: cpp 92/93, c 87/93, interp 74/93);
  3. `tests/corpus` (190 `.dsp` fixtures) with parity scan reports;
  4. `tests/cpp_parity_known_gaps` — minimal reproducers with an explicit
     promotion process into the corpus, giving auditable provenance for
     parity fixes.
- Runtime traces (`tests/runtime_traces`) and interp trace-diff tooling for
  the interpreter backend.
- Test-to-code ratios are healthy where it matters: compiler 336 tests,
  codegen 261, eval 111, transform 104, fir 98, propagate 90.

Gaps: no coverage measurement is wired into CI (a periodic `cargo llvm-cov`
report would locate the untested corners of `codegen`'s 36.8 k lines), and the
interp impulse baseline (74/93) trails c/cpp — tracked, but the gap list is
only in memory/journal rather than a checked-in status file.

---

## 5. Documentation quality

### 5.1 Strengths — well above typical for a port

- **C++ provenance convention**: 73 source files carry explicit
  "Source provenance (C++)" / "C++ correspondence" sections naming the
  original file and function, often with behavior-comparison tables (e.g.
  `eval/loop_detector.rs` tabulates C++ `set<Tree>` vs Rust `Vec<TreeId>`
  with complexity trade-offs; `eval/lib.rs` documents the environment model
  against C++ property lists operation by operation). This is exactly what a
  port needs and must be preserved as a review requirement for new code.
- **Design memory**: 111 dated analysis/plan documents in `porting/`
  (`<topic>-<date>-en.md`), plus a journal split into 100 daily files with an
  indexed README. Decisions are recoverable years later — rare and valuable.
- Doc-comment density in the front/middle end is high: eval 26 %,
  propagate 22 %, transform 18 % of lines are doc comments.

### 5.2 Weaknesses

- **Changelog-headers**: `transform/src/signal_fir/mod.rs` opens with ~45
  lines of "Step 2A/2B/…/2H, RAD Phase B3/B4/B5" accretion — a development
  log, not a description of the module as it stands. A reader must mentally
  replay the history to learn the current contract. The journal already holds
  this history; the header should be rewritten as an architecture overview
  (inputs, outputs, invariants, module map). Same pattern threatens other
  fast-moving modules.
- **No index over `porting/`**: 111 documents, some superseded by later ones
  (e.g. the 2026-03-24 propagation plan superseded by the 2026-07-04 one),
  with no README distinguishing *active plan* / *implemented* / *superseded*.
  The journal has an index; the design docs need the same.
- **Doc density is uneven toward the back end**: fir 8.7 %, codegen 9.8 %,
  sigtype 12.7 % vs eval 26 %. The backends are where new contributors will
  land (adding a language), and they are the least documented layer.
- **`missing_docs` is not enforced anywhere** — density is maintained by
  culture only. Enabling `#![warn(missing_docs)]` on the stable foundation
  crates (`tlib`, `errors`, `interval`, `boxes`, `signals`) would lock in the
  current standard at near-zero cost.
- **`CODEGUIDELINES.md` is written for TypeScript projects** (its examples are
  TS, its rules reference branded types). The ADT-first philosophy visibly
  *did* shape the Rust code, but the document itself doesn't match the
  language of the codebase and can confuse tooling and contributors alike.
- `JOURNAL.md` (105 lines) vs `porting/journal/` (split snapshot at commit
  `4eebb49`) — the relationship (which is authoritative, when is the split
  regenerated) is undocumented.

---

## 6. Prioritized improvements

| # | Item | Effort | Impact |
|---|---|---|---|
| 1 | **Propagation memoization** (P0–P2 of the [dedicated plan](cpp-propagate-eval-memoization-port-plan-2026-07-04-en.md)) — the ~5× compile-time gap is the port's largest measured deficiency | M | High |
| 2 | **Rewrite accreted module headers** (`signal_fir/mod.rs` first) into current-state architecture docs; move step history pointers to the journal | S | High (onboarding) |
| 3 | **`porting/README.md` index** classifying the 111 docs (active / implemented / superseded), and state the JOURNAL.md ↔ journal-split relationship | S | Medium |
| 4 | **Audit the 54 non-test `.expect()` in `propagate`**: convert cross-phase contract expectations (control-id registry, seed bookkeeping) into typed `PropagateError` variants; keep same-function decode guards as panics | S–M | Medium (robustness) |
| 5 | **Shared C-family emitter core** for c/cpp (then julia): parameterize syntax, keep per-language modules thin; the `backend-align-smoke` reports become the migration safety net | M–L | Medium (removes drift class) |
| 6 | **Split remaining god files**: `fir/checker.rs`, `fir/inliner.rs`, `codegen/backends/wasm/mod.rs`, `box-ffi/lib.rs`, and carve `signal_fir/tests.rs` into per-feature test modules | M | Medium |
| 7 | **`#![warn(missing_docs)]`** on stable foundation crates; raise back-end doc density opportunistically when touching backends | S | Medium |
| 8 | **Replace `CODEGUIDELINES.md`** with a Rust-native version preserving the ADT/opacity/two-questions-documentation principles the codebase already follows | S | Low–Medium |
| 9 | **Coverage report** (`cargo llvm-cov`) as a periodic xtask/CI artifact, focused on `codegen`; check in an interp impulse-gap status file next to the baselines | S | Low–Medium |
| 10 | **Scaffold visibility**: one workspace-level table (README) of implemented vs placeholder crates/backends; delete scaffolds that no longer serve the roadmap (e.g. `doc`, `graph` if `petgraph`-style needs never materialized) | S | Low |

(Effort: S < 1 day, M = days, L = weeks.)

---

## 7. Bottom line

As a **port**, faust-rs is exemplary on the two axes that matter most:
*behavioral fidelity* (four-level differential validation against the C++
compiler, parity-gap bookkeeping with promotion discipline) and *architectural
translation* (it chose Rust-native designs that upstream C++ is now adopting
back). Code hygiene is enforced mechanically, not aspirationally. The
weaknesses are those of a fast-moving single-author project: history-shaped
documentation in the newest layers, duplicated emitter logic, oversized files
in the back end, and a design-document corpus that has outgrown its lack of an
index. All are incremental to fix; none are structural. The single measured
functional deficiency versus C++ remains propagation-phase compile time, which
has its own implementation plan.
