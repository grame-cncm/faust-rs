# faust-rs — Implementation Quality Assessment

Date: 2026-05-25
Reviewer perspective: senior engineer, independent audit
Scope: full workspace (`crates/`), porting docs, test infrastructure, C++ parity
Reference C++ baseline: `master-dev-ocpp-od-fir-2-FIR19` (`8eebea429`)

---

## 1. Executive Summary

`faust-rs` is a **mature, unusually disciplined port** of the Faust compiler to
Rust. After ~3 months of intense single-author development (1094 commits, 182 in
the last 30 days), it reaches **front-end parity** with the pinned C++ reference
on the tracked corpus and **zero-divergence** output on 93/94 portable backend
cases. It compiles cleanly with **zero warnings**, **zero clippy lints**, a green
test suite (~1387 tests), and only **one** inline debt marker in 131 kLOC. It also
ships features that **do not exist in upstream Faust** — forward- and reverse-mode
automatic differentiation (`fad`/`rad`).

The project's weaknesses are not about correctness or hygiene — both are
excellent. They are about **structural granularity and scope discipline**:

- a handful of **"god files / god structs"** concentrate enormous logic
  (5020-line `module.rs` with a 127-method `SignalToFirLower`; 4946-line
  `eval/lib.rs`; 4599-line `compiler/lib.rs`);
- **premature structure**: 3 unused scaffold crates and 10 stub backends carry
  name/shape but no implementation;
- a **backend layer that leans on `.unwrap()`** (188 in `codegen`) rather than
  typed diagnostics;
- **doc/process drift**: an orphaned `CODEGUIDELINES.md` (written for TypeScript),
  a stale "scaffold placeholder" header on an active crate, and 220 untracked
  scratch files cluttering the repo root.

**Verdict:** production-quality core engineering with localized refactoring debt.
The single highest-value technical investment ahead is the **reverse-mode AD
re-architecture** (linearize-once + transpose), already planned. The
highest-value *low-effort* investments are factorization of the 3–5 largest files
and repo/scope housekeeping.

---

## 2. Project Snapshot

| Metric | Value |
|---|---|
| Workspace crates | 27 |
| Rust source (crates/*/src) | ~131,230 LOC |
| Test source (crates/*/tests) | ~16,552 LOC |
| `#[test]` functions | ~1,387 (4 `#[ignore]`) |
| Corpus DSP inputs | 194 |
| Golden reference outputs | 220 |
| Commits | 1,094 (2026-02-14 → 2026-05-22) |
| Commit rate (last 30d) | 182 |
| Contributors | 1 |
| Version / edition | 0.5.0 / Rust 2024 |
| `cargo build --workspace` | clean, 0 warnings |
| `cargo clippy --workspace` | 0 warnings |
| Full test suite | green |
| Inline debt markers (TODO/FIXME/HACK) | 1 |

Backends present (by implementation size):

| Backend | LOC | Status |
|---|---|---|
| interp (FBC VM) | 16,029 | real |
| cranelift (JIT) | 5,611 | real |
| wasm | 5,489 | real (subset) |
| julia | 1,760 | real (first slice) |
| cpp | 1,755 | real |
| c | 1,670 | real |
| vhdl, sdf3, rust, llvm, jsfx, jax, dlang, csharp, codebox, cmajor | 17 each | **stub only** |

---

## 3. Strengths (what is working well)

These are genuine, measurable strengths and should be preserved as the project
evolves.

1. **Quality gates are real and enforced.** Clean build, zero clippy warnings, a
   green ~1387-test suite, and an `AGENTS.md`-mandated gate (`fmt` +
   `clippy -D warnings` + `test --all-targets`) on every porting step. This is
   rare for a port of this size and velocity.

2. **Safety architecture is deliberate.** All 20 pure-Rust crates opt into
   `[workspace.lints]` which sets `unsafe_code = "forbid"`. The 6 FFI/boundary
   crates (`*-ffi`, `foreign-call`, `utils`) deliberately *omit* the opt-in so
   they can use `unsafe` at the C ABI boundary only. The unsafe surface is thus
   confined to where it is unavoidable (cranelift-ffi 280, interp-ffi 159,
   box-ffi 134 occurrences) and forbidden everywhere else.

3. **Clean dependency DAG.** No cycles. Layering tracks the Faust pipeline
   exactly: `tlib/errors/interval` (L0) → `ui/fir/boxes` → `signals/draw/codegen`
   → `sigtype/propagate/parser` → `normalize` → `transform` → `eval` →
   `compiler` → FFI/`xtask`. This matches the canonical
   `parse → boxes → eval → propagate → normalize → type/interval → transform →
   fir → backend` flow.

4. **Differential testing against C++ is institutionalized.** 194 corpus inputs,
   220 golden snapshots (`tests/golden/rust` for the CI gate, `tests/golden/cpp`
   for the long-run parity target), plus runtime traces and an `xtask
   golden-check` guardrail. Correctness is measured against the real compiler,
   not asserted.

5. **Documentation discipline in the core is high.** Provenance Rustdoc
   referencing C++ sources is mandated and present; `module.rs` carries 849
   doc-comment lines, `eval/lib.rs` 903. Every field of the central
   `SignalToFirLower` struct documents its role *and* its invariants
   (`crates/transform/src/signal_fir/module.rs:875`).

6. **Debt is externalized, not hidden.** Only one inline `HACK` exists in all of
   `src`. Known gaps live in `porting/` plans and `JOURNAL.md` per the
   `AGENTS.md` policy — far more reviewable than scattered `// TODO`s.

7. **It exceeds upstream Faust in one strategic dimension:** in-graph forward and
   reverse automatic differentiation, a differentiating capability with no
   counterpart in the C++ compiler.

---

## 4. Architecture Assessment

### 4.1 What is sound

The crate decomposition is the project's structural backbone and it is correct:
single-responsibility crates, a clean DAG, and a layering that a Faust developer
would recognize immediately. The `eval`/`propagate`/`normalize`/`transform`
split mirrors the C++ compiler's conceptual stages while adapting ownership and
error handling idiomatically (the `adapted` vs `1:1` policy in `AGENTS.md` §5 is
the right call).

The fast-lane design (`SignalFirLane::TransformFastLane`) as the production route,
with the broader front-end language surface layered above it, is a pragmatic way
to ship a working backend while the long tail of signal families is filled in.

### 4.2 Premature / dead structure (cleanup target)

Three workspace crates are pure scaffolds that **nothing depends on**:

- `crates/algebra` (28 LOC) — only `crate_id()`
- `crates/graph` (20 LOC) — only `crate_id()`
- `crates/doc` (21 LOC) — only `crate_id()`

And 10 backend directories are 17-LOC stubs (`vhdl`, `sdf3`, `rust`, `llvm`,
`jsfx`, `jax`, `dlang`, `csharp`, `codebox`, `cmajor`).

These mirror the *shape* of the C++ backend/pass inventory but carry no behavior.
They are low-cost individually, but collectively they inflate the apparent
surface area and can mislead a newcomer about what is implemented. **Recommendation:**
either (a) delete them until there is an active plan to fill them, or (b) keep
them but gate them behind a documented "reserved namespace" note and exclude them
from the crate-count narrative. Today they sit in an ambiguous middle.

### 4.3 C++ fidelity

The port is faithful where it matters (interval algebra, signal-type lattice,
recursion model via `sigRec/sigProj`, delay sizing from interval upper bounds)
and idiomatic where it should be (typed errors, ownership). The one notable
*divergence by design* — operating FAD directly on De Bruijn recursive form and
interleaving primal/tangent slots before symbolic conversion — is documented and
test-backed.

---

## 5. Code Quality Assessment

### 5.1 The headline issue: file & struct granularity

The core logic is concentrated in a small number of very large units:

| File | LOC | Shape |
|---|---|---|
| `transform/src/signal_fir/module.rs` | 5,020 | 1 struct `SignalToFirLower`, ~127 methods, 20+ fields |
| `eval/src/lib.rs` | 4,946 | 150 free fns |
| `compiler/src/lib.rs` | 4,599 | 185 fns, 18 types — orchestration facade |
| `xtask/src/main.rs` | 4,356 | 124 fns, only 173 doc lines |
| `propagate/src/lib.rs` | 3,740 | 74 fns |
| `box-ffi/src/lib.rs` | 2,709 | single-file C ABI |

`SignalToFirLower` is the textbook example: a single struct that owns the type
maps, the recursion state, the delay manager, *and* the six per-lifecycle
statement vectors (`constants_statements`, `reset_statements`, `clear_statements`,
`control_statements`, `sample_phases`, …), with ~127 methods spanning dispatch,
delay/recursion helpers, constant/UI/soundfile/table lowering, and arithmetic.
It is **well-documented and clearly sectioned** (the file uses `// ── … ──`
dividers), so this is not sloppy code — it is *monolithic* code. The cost is in
testability (methods can only be exercised through the whole lowerer), merge
contention, and onboarding.

This is recognized debt: the journal already records `boxes`/`eval` module splits
on 2026-03-24. The work is simply unfinished for the largest remaining units.

**Recommendation (incremental, behavior-preserving):**
- Extract cohesive method clusters of `SignalToFirLower` into sibling modules via
  `impl` blocks in separate files (`module/delay.rs`, `module/tables.rs`,
  `module/arithmetic.rs`, `module/lifecycle.rs`) — Rust allows splitting one
  `impl` across files in the same module, so no API change is needed.
- Split `eval/lib.rs`'s 150 free fns by concern (already started; finish it).
- Treat `compiler/lib.rs` as a thin façade: it should *compose* stage entry
  points, not contain stage logic (cf. the project's own R6 "declarative
  composition" principle in `CODEGUIDELINES.md`).

### 5.2 Robustness: backend `unwrap` density

The backend layer relies heavily on panicking accessors:

| Crate | `.unwrap()` | `.expect()` | `panic!` | `unreachable!`/`debug_assert` |
|---|---|---|---|---|
| codegen | 188 | 119 | 17 | 1 / 2 |
| transform | 0 | — | — | (357 panic-family total, mostly guards) |
| eval | 15 | — | — | 26 |
| sigtype | 0 | — | — | 10 (clean) |

`codegen`'s 188 raw `.unwrap()` are mostly downstream of a validated FIR module,
so in practice they assert invariants rather than handle user input. But a raw
`.unwrap()` panics with no context; if an upstream regression ever produces a
malformed-but-accepted FIR, the failure mode is an opaque panic instead of a
diagnostic. **Recommendation:** audit `codegen` `.unwrap()` sites and convert the
load-bearing ones to `.expect("invariant: …")` (cheap, documents the assumption)
or to typed `LowerError` where the condition is genuinely reachable. `transform`'s
panic-family count is high but is dominated by invariant guards (only 1
`unreachable!`, 2 `debug_assert` in `codegen` by contrast) — verify the same for
`transform`.

### 5.3 Documentation / process drift

- **`CODEGUIDELINES.md` is orphaned and mistargeted.** It is titled "Code
  Structure Guidelines for **TypeScript** Projects" and every example is
  TypeScript, in a workspace with zero TypeScript. It is referenced only by
  agent worktree handoffs, never by the main repo. The *principles* (ADT-per-file,
  opacity, DAG, two-line doc, declarative composition) are sound and largely
  honored in spirit, but the document as written is misleading. **Recommendation:**
  rewrite it for Rust (or delete it and fold the principles into `AGENTS.md`,
  which is already the real, excellent guide).
- **Stale crate header.** `crates/utils/src/lib.rs` still opens with "Shared
  utility crate **placeholder** … Scaffold only" while shipping 527 LOC of
  actively-used FFI types. Update the header.

### 5.4 Working-tree hygiene

`git status` shows **220 untracked entries at the repo root**: 171 `*.dsp`
experiments (`fad_*`, `rad_*`, `t1`–`t15`, `*_gemini*`), 18 generated `*.cpp`,
5 `*.patch`, plus binaries (`fad_delay`, `fad_spat4`). The tracked tree is clean
(0 modified), so this is pure scratch clutter. **Recommendation:** move scratch
DSPs to a gitignored `scratch/` (or `examples/ad/`) directory and add
`*.patch`, generated `*.cpp`, and built binaries to `.gitignore`. This also
de-risks accidental commits of large generated files (several `*.cpp` are >1 MB).

---

## 6. C++ Parity Status

Sourced from `porting/faust-rs-supported-faust-subset-en.md` (2026-05-20) and
cross-checked against the crate layout.

### 6.1 Front-end — at corpus parity

On the 190-case tracked corpus: **94 OK/OK**, **18 ERR/ERR**, **0 OK/ERR
mismatches**, **78 ERR/OK** (Rust-only `fad`/`rad` extensions the pinned C++
rejects as undefined symbols). In short, `faust-rs` accepts every portable source
the C++ reference accepts up to the `signals` boundary, rejects the same invalid
ones, and adds the AD extensions. This is the strongest part of the port.

### 6.2 Backends — zero divergence, narrower coverage

C and C++ end-to-end: **OK=93, DIFF=0, UNSUPPORTED=97**. The `DIFF=0` is the key
number: where Rust compiles, the output matches C++. Of the portable C++-accepted
set, **93/94** compile end-to-end; the one miss is `rep_18_stream_wrappers.dsp`.

- **WASM + JSON:** real `-lang wasm`/`-lang wast` path, companion JSON coherent
  with `faustwasm` on validated cases (`osc.dsp`, polyphonic `organ.dsp`); not yet
  byte-for-byte UI-offset parity.
- **Julia:** functional first slice (`-lang julia`), architecture wrapping,
  `-double` propagation, cast/math helpers; not yet full impulse-test coverage.

### 6.3 Where C++ remains broader

- Specialized **reverse-time recursion AD** (delay/prefix/recursion in the
  differentiated body) — currently routed through the `BlockReverseAD` fallback;
  LTI transposition (phase E1) and BPTT (phase F) are future work.
- Stream-wrapper lowering (`rep_18`).
- `tabulateNd` / multi-dimensional tables (the FIR table-size extractor still
  requires a literal `Int` node).
- Long-tail signal families, mature Julia runtime packaging, and a fuller
  embedded-compiler helper surface (`getInfos` only partial).

The previously large **variable-delay** gap is now substantially closed and is, by
the document's own analysis, a code-quality difference (`fOutDelayOcc`
optimization) rather than a correctness gap.

---

## 7. Special Focus — the AD Subsystem (the design frontier)

Automatic differentiation is the project's most ambitious feature and its most
actively churning subsystem. It is also where the most design risk and the most
consolidation opportunity live.

**Footprint (cross-cutting):**

- `propagate/forward_ad.rs` (1,754) — forward-mode (`fad`)
- `propagate/reverse_ad.rs` (1,265) — feed-forward symbolic reverse (`rad`)
- `propagate/transpose_ad.rs` (1,076) — transpose direction
- `propagate/stateful_rad.rs` (1,069) — stateful reverse
- `signals/ad_rules.rs` — shared, backend-neutral classification/formula table
- `transform/signal_fir/block_reverse_ad.rs` — `BlockReverseAD` temporal fallback

**Assessment.** Forward-mode AD is in good shape: 35 corpus entries validated
through the fast-lane, operating natively on De Bruijn recursive form. Reverse-mode
is the harder problem and currently carries **several overlapping mechanisms** —
symbolic feed-forward RAD, `BlockReverseAD`, a transpose path, a stateful path,
plus dormant E0 (recursive-linearity classification) and E1 (LTI transposition)
scaffolds. The 2026-05-17 work to share one local-rule table
(`signals::ad_rules`) between the Signal-level and FIR/BRA paths is exactly the
right consolidation instinct.

The team has already analyzed the target architecture: **linearize-once +
transpose** (the "YOLO" direction), documented in
`porting/rad-linearize-once-transpose-plan-2026-05-21-en.md` and
`porting/yolo-linearize-once-rad-analysis-2026-05-21-en.md`. The key design
insight recorded there — separating primal-value tape/recomputation (driven by
primal-dependent local Jacobians) from temporal adjoint carries and reverse-time
scheduling — is sound and matches how JAX (`linearize` + `backward_pass`
transpose) and the AD literature structure the problem.

**Recommendation:** prioritize this re-architecture. The payoff is not just a new
capability (reverse-time recursion) but **retirement of redundant AD-specific FIR
machinery** once a generic reverse-time region + cross-loop cache exist. Until
then, the four `propagate/*_ad.rs` files plus the BRA fallback are the part of the
codebase most likely to accumulate subtle parity bugs, so keep the differential
gradient tests (TBPTT convergence suite in `rad_runtime.rs`) as the gate.

---

## 8. Prioritized Recommendations / Next Steps

Ordered by value/effort. Items 1–3 are the strategic technical roadmap (already
partly planned); items 4–8 are high-leverage engineering hygiene.

### Tier 1 — Strategic (weeks)

1. **Execute the reverse-mode AD re-architecture** (linearize-once + transpose).
   Stage feed-forward first, keep `SigBlockReverseAD` as the temporal carrier
   during transition, then collapse the redundant mechanisms. Gate with the
   existing TBPTT convergence tests. *(Plan exists; this is the single biggest
   technical lever.)*

2. **Close the named backend gaps.** `rep_18_stream_wrappers` (the last portable
   C++-accepted miss) and `tabulateNd`/multi-dimensional tables (needs the FIR
   table-size extractor to accept non-literal sizes now that `simplify` runs in
   `signal_prepare`).

   Follow-up (2026-08-11): the `tabulateNd`/computed-table-size part is closed;
   global simplification now reduces the extent before FIR extraction and
   `rep_87_table_computed_size` maintains the behavior. The stream-wrapper
   numerical difference remains tracked separately.

3. **Decide WASM/Julia maturity targets.** Either commit to byte-parity WASM UI
   offsets + full Julia impulse-test coverage, or explicitly document them as
   "functional subset, parity not pursued" so the scope is honest.

### Tier 2 — Factorization (days, behavior-preserving)

4. **Split the 3 largest units.** `SignalToFirLower` (`module.rs`) into
   per-concern `impl` files; finish `eval/lib.rs`; reduce `compiler/lib.rs` to a
   composition façade. No public-API change required; lock with the existing
   golden gate.

5. **Harden `codegen`.** Audit the 188 `.unwrap()` / 119 `.expect()`; convert
   load-bearing ones to documented `.expect("invariant: …")` or typed
   `LowerError`. Do the same pass on `transform`.

### Tier 3 — Housekeeping (hours)

6. **Resolve premature structure.** Delete or formally "reserve" the 3 unused
   scaffold crates (`algebra`, `graph`, `doc`) and the 10 stub backends; whichever
   you choose, make the status unambiguous in the README crate table.

7. **Fix doc drift.** Rewrite `CODEGUIDELINES.md` for Rust or fold it into
   `AGENTS.md`; update the stale `utils` "placeholder" header.

8. **Clean the working tree.** Move 171 scratch `*.dsp` to a gitignored
   directory; gitignore `*.patch`, generated `*.cpp`, and built binaries.

### Process note

This codebase has been carried by a single author at very high velocity. The
externalized-debt discipline (porting/ + JOURNAL) is excellent for solo work but
becomes a **bus-factor risk** at this scale. The factorization in Tier 2 and the
scope clarity in Tier 3 are also the cheapest ways to make the project
contributor-ready, should that become a goal.

---

## 9. Bottom Line

| Dimension | Grade | Note |
|---|---|---|
| Correctness vs C++ | A | 0 OK/ERR front-end, 0 DIFF backend |
| Build/lint/test hygiene | A | 0 warnings, 0 clippy, green suite, 1 debt marker |
| Safety model | A | forbid-unsafe core, confined FFI unsafe |
| Architecture / DAG | A− | clean layering; minor dead/premature structure |
| Documentation (core) | A− | strong provenance docs; one orphaned/mistargeted guide |
| Factorization | C+ | a few god files/structs; recognized, partly addressed |
| Backend robustness | B− | `codegen` unwrap density |
| Scope discipline | B | stub backends, scaffold crates, scratch clutter |
| AD subsystem | B | powerful and unique; mid-re-architecture, overlapping paths |

`faust-rs` is a high-quality, correctness-first port whose remaining work is
**consolidation, not construction**: finish the AD re-architecture, split the
largest files, harden the backend's failure modes, and tidy scope. The
engineering fundamentals are already in place and well above average for a port of
this scale and age.
