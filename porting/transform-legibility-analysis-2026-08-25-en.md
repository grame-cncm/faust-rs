# Analysis: making `crates/transform` simpler for a human to read

**Date:** 2026-08-25
**Scope:** the whole `crates/transform` crate (150 `.rs` files), with `signal_fir/` as the main
subject. Analysis only — no code change is proposed for immediate landing; each experiment below
is an independently landable, behavior-preserving campaign.
**Status:** proposed; **E1 executed 2026-08-25** on branch
`transform-legibility-e1` (commits `9fa67e0e` loop_graph truth fix + dead
cluster, `41c83228` diagnostics quarantine, `a6bb1c6d` codename sweep +
`structure-check` gates 6/7 + reading order — see the 2026-08-25 journal).
E2 and E3 remain proposed. E1 also surfaced one follow-up: the scalar-side
`-vec` chunk-driver machinery (`emit_sample_loop`'s vector branch and the
chunking half of `loop_graph.rs`) is provably unreachable in production and
can be deleted behavior-neutrally (separate task).
**Goal:** identify why the crate is still costly for a *human* to read after the June 2026
decompositions and the July 2026 R0–R9 cleanup, and propose three **independent** restructuring
experiments that reduce reading cost while keeping **byte-identical emitted FIR ⇒ identical
generated C/C++/WASM ⇒ identical performance**.
**French twin:** [`transform-legibility-analysis-2026-08-25-fr.md`](transform-legibility-analysis-2026-08-25-fr.md)
(this English version is canonical).
**Companion docs:**
[`signal-to-fir-transform-analysis-2026-06-20-en.md`](signal-to-fir-transform-analysis-2026-06-20-en.md)
(step-by-step pipeline walk; still the best "what happens in what order" reference),
[`transform-cleanup-documentation-factorization-plan-2026-07-19-en.md`](transform-cleanup-documentation-factorization-plan-2026-07-19-en.md)
(the executed R0–R9 structural cleanup this analysis builds on),
[`delay-rs-simplification-experiment-2026-06-21-en.md`](delay-rs-simplification-experiment-2026-06-21-en.md) and
[`signal-prepare-simplification-experiment-2026-06-22-en.md`](signal-prepare-simplification-experiment-2026-06-22-en.md)
(the two previous legibility experiments, both implemented; same method applied here at crate scale).

---

## 0. Position in the compilation chain (recap)

```
boxes ──► propagate ──► signals (+ UiProgram)
                            │
                            ▼
        ┌────────────────────────────────────────────────────────┐
        │ crates/transform                                        │
        │                                                         │
        │  signal_prepare ──► clk_env / hgraph / schedule ──►     │
        │    (staging)          (analysis)                        │
        │                       signal_fir ──► FIR                │
        │                    (scalar + checked vector lowering)   │
        └────────────────────────────────────────────────────────┘
                            │
                            ▼
              fir ──► codegen (C / C++ / WASM / Cranelift / FBC)
```

The pipeline itself is documented well in
[`lib.rs`](../crates/transform/src/lib.rs) and in the 2026-06-20 analysis; this document does not
repeat it. The single production entry point is
[`compile_signals_to_fir_fastlane`](../crates/transform/src/signal_fir/mod.rs:615) driven by the
`SignalFirRequest` builder ([mod.rs:481](../crates/transform/src/signal_fir/mod.rs:481)) — itself
the result of a good 2026-08-18 legibility fix (five near-identical entry points collapsed into
one request struct; the doc comment there tells the story).

## 1. Measured state (2026-08-25, `main-dev`)

All numbers were measured on this date; the measurement commands are given so the tables can be
re-derived after any experiment lands.

### 1.1 Crate scale

`transform` is now the **largest crate of the workspace** (`python3 scripts/loc_report.py
--by-crate`, cloc-based, blank/comment lines excluded):

| Crate | Effective LOC | Test LOC | Total |
|---|---:|---:|---:|
| **transform** | **34,628** | **15,681** | **50,309** |
| codegen | 32,717 | 11,839 | 44,556 |
| compiler | 11,536 | 18,208 | 29,744 |

Raw line counts (`wc -l`, comments and blanks included): 62,399 lines over 150 files.

### 1.2 Where the lines live (raw `wc -l` per sub-tree)

| Sub-tree | Raw lines | Share | Note |
|---|---:|---:|---|
| `signal_fir/vector/` | 29,915 | 48 % | checked vector pipeline (11 stages × model/build/check/tests) |
| `signal_fir/` root files | 8,913 | 14 % | `mod.rs`, `loop_graph`, `cse`, `decoration_verify`, `recursion`, `pv_slice`, `shadow`, … |
| `signal_fir/module/` | 8,095 | 13 % | scalar lowerer (14 files) |
| `signal_fir/tests/` | 5,152 | 8 % | scalar-path tests, already split by concern |
| `signal_prepare/` | 3,026 | 5 % | staging + verifier |
| `schedule/` | 2,548 | 4 % | generic `-ss` scheduler |
| `signal_fir/delay/` | 2,252 | 4 % | June 2026 layout, still sound |
| `hgraph/` + `clk_env/` | 2,437 | 4 % | analysis stages |

### 1.3 Function-level scale — the main finding

A brace-balance scan over `src/` counts **1,589 functions**, of which **69 exceed 100 lines and 21
exceed 200 lines**. The algorithmic heart of the crate is concentrated in these functions:

| Lines | Function | Location |
|---:|---|---|
| 767 | `build_module` | [`module/build.rs:541`](../crates/transform/src/signal_fir/module/build.rs:541) |
| 643 | `verify_vector_plan` | [`vector/verify/check.rs:49`](../crates/transform/src/signal_fir/vector/verify/check.rs:49) |
| 585 | `build_vector_plan` | [`vector/plan/build.rs:105`](../crates/transform/src/signal_fir/vector/plan/build.rs:105) |
| 557 | `verify_prepared_signal` | [`signal_prepare/verify.rs:127`](../crates/transform/src/signal_prepare/verify.rs:127) |
| 536 | `build_fused_serial_groups` | [`vector/plan/fusion.rs:21`](../crates/transform/src/signal_fir/vector/plan/fusion.rs:21) |
| 447 | `verify_fused_serial_groups_after_plan` | [`vector/verify/fused_groups.rs:30`](../crates/transform/src/signal_fir/vector/verify/fused_groups.rs:30) |
| 345 | `lower_raw` | [`vector/lower/signal.rs:573`](../crates/transform/src/signal_fir/vector/lower/signal.rs:573) |
| 305 | `lower_vector_program_impl` | [`vector/lower/signal.rs:156`](../crates/transform/src/signal_fir/vector/lower/signal.rs:156) |
| 300 | `propagate_bra_adj` | [`module/bra.rs:429`](../crates/transform/src/signal_fir/module/bra.rs:429) |
| 289 | `compile_fastlane_inner` | [`signal_fir/mod.rs:648`](../crates/transform/src/signal_fir/mod.rs:648) |
| 277 | `ensure_guarded_block` | [`module/clocked.rs:487`](../crates/transform/src/signal_fir/module/clocked.rs:487) |
| 274 | `lower_signal` | [`module/core_lowering.rs:96`](../crates/transform/src/signal_fir/module/core_lowering.rs:96) |
| 261 | `infer_uncached` | [`clk_env/mod.rs:431`](../crates/transform/src/clk_env/mod.rs:431) |
| 260 | `materialize_action` | [`vector/assemble/materialize.rs:572`](../crates/transform/src/signal_fir/vector/assemble/materialize.rs:572) |
| 258 | `lower_proj` | [`module/arithmetic.rs:277`](../crates/transform/src/signal_fir/module/arithmetic.rs:277) |

19 non-test files still exceed 800 raw lines.

### 1.4 The twin production lowerers

Both lowerers already dispatch on the typed
[`SigMatch`](../crates/signals/src/lib.rs:1128) view (`match_sig`),
so this is *not* raw-tree spaghetti — but the crate contains **two complete production
signal→FIR dispatchers**:

- scalar: `lower_signal` (274 lines) plus arm helpers spread over 9 of the 14 `module/` files;
- vector: `lower_raw` (345 lines) plus helpers in `vector/lower/signal.rs` (2,339 lines, the
  largest file of the crate).

210 `match_sig(` call sites exist crate-wide. The stateless arm families (numeric constants,
binops, unary/binary math, casts, min/max/pow) are emitted **twice with identical FIR shapes**;
sharing has already started for exactly one item
([`map_binop`](../crates/transform/src/signal_fir/module/arithmetic.rs) is imported by
`vector/lower/signal.rs:13`) but stops there.

The two lowerer state structs are large despite the June sub-state extraction:

- `SignalToFirLower` ([`module/mod.rs:343`](../crates/transform/src/signal_fir/module/mod.rs:343)):
  ~35 fields, 7 of them already grouped sub-states (the table at `mod.rs:330` documents them);
- `PureVectorLowerer` ([`vector/lower/signal.rs:48`](../crates/transform/src/signal_fir/vector/lower/signal.rs:48)):
  ~40 **flat** fields with no equivalent grouping, plus a `(scope, sig, cache, active)` parameter
  quadruple threaded through nearly every method signature.

### 1.5 Documentation state: excellent maps, drifting leaves

Strengths that must be preserved:

- module headers of [`clk_env`](../crates/transform/src/clk_env/mod.rs),
  [`hgraph`](../crates/transform/src/hgraph/mod.rs),
  [`schedule`](../crates/transform/src/schedule/mod.rs) and the authoritative stage-map table of
  [`vector/mod.rs`](../crates/transform/src/signal_fir/vector/mod.rs) are genuinely good teaching
  material;
- the 10 `Verified*` artifact types make the producer/checker chain visible in the type system;
- `#![deny(missing_docs)]` ([`lib.rs:46`](../crates/transform/src/lib.rs:46)) keeps every `pub`
  item documented.

Two measured weaknesses:

**(a) Plan-codename indirection.** ≈200 comment lines (grep for
`P[0-9]\.[0-9]|R[0-9]|V[0-9]|S[0-9]|§[0-9]|Step 2[A-H]`) describe present-tense code in the
coordinates of historical porting plans ("roadmap P6, vector doc V2", "P4.3b", "§4.8", "Step
2A..2G", "S6"). The glossary at `vector/mod.rs` §"Plan-codename glossary" mitigates this for the
vector tree only; everywhere else the reader needs `porting/` history to parse a doc comment.

**(b) Truth drift, one concrete instance.**
[`loop_graph.rs`](../crates/transform/src/signal_fir/loop_graph.rs:21) still claims *"Nothing here
is wired into scalar codegen yet, so it cannot affect existing output; the `dead_code` allowance
is removed when V3 starts populating it"* — but
[`module/build.rs:991`](../crates/transform/src/signal_fir/module/build.rs:991) routes **every
scalar per-sample slice** through `LoopGraph` today. Replacing the file-level
`#![allow(dead_code)]` (line 23) with nothing produces exactly 11 warnings, i.e. the allowance
now hides a real dead cluster: `LoopKind::Island`, `is_vectorizable`, `len`/`is_empty`/`add_dep`,
`loop_kind`, `LoopAssignment`, `loop_of`, `signal_value_children`, `assign_loops`, `assign_one`,
`name` — the loop-assignment half of the file, superseded by `vector/plan/`.

### 1.6 Diagnostic surfaces mixed into the production tree

Two observation-only modules live undistinguished next to production code:

- [`pv_slice.rs`](../crates/transform/src/signal_fir/pv_slice.rs) (680 lines, P2 pre-slice
  diagnostic): consumed only by `crates/compiler/tests/pv_vector_slice.rs` and in-crate tests;
- [`shadow.rs`](../crates/transform/src/signal_fir/shadow.rs) (schedule-conformance reports):
  consumed only by `crates/compiler/tests/p3_shadow_mode.rs` and the `FAUST_RS_SHADOW_REPORT`
  environment variable; its plumbing (`emission_order`, `emission_seen`, `shadow_report`) threads
  through `SignalToFirLower` and `SignalFirOutput`.

A reader walking the production path cannot tell, without reading each header, which neighbors
are load-bearing.

## 2. Diagnosis: what still costs a human reader

The June/July campaigns fixed the *module-level* story: files are split by concern, stages have
maps, tests are separated. What remains expensive is one level below and one level above:

- **L1 — function-level scale.** The 21 functions over 200 lines are where the real algorithms
  live, and inside them there is no named narrative unit. `build_module` (767 lines) is the
  clearest case: the whole scalar module assembly is one function.
- **L2 — twin-lowerer duplication.** Two production dispatchers repeat the stateless leaf
  emission arm-by-arm. Cost is double reading and a real drift risk (a fix applied to one path
  only), *not* an intentional assurance boundary — the producer/checker doctrine of
  `vector/mod.rs` protects checkers, not two producers.
- **L3 — codename indirection.** Doc comments speak in plan coordinates; the code describes
  its own history instead of its present behavior.
- **L4 — truth drift.** One measured stale header + one blanket `allow` hiding a dead cluster
  (§1.5b). Each instance erodes trust in the otherwise excellent headers.
- **L5 — production/diagnostic mixing.** §1.6.

## 3. Three independent restructuring experiments

Each experiment is independently landable, FIR-neutral, and mechanically gated. They are ordered
cheapest-first; §5 gives the shared migration protocol.

### E1 — Present-tense documentation and truth maintenance (targets L3, L4, L5)

Zero-FIR-risk by construction (comments, moves, dead-code deletion).

1. **Semantic names over codenames.** Every doc comment that currently *explains behavior* via a
   plan codename is rewritten to explain the behavior in its own words; provenance is kept, but
   demoted to a trailing `Plan provenance:` line (or the per-tree glossary, extending the
   `vector/mod.rs` model to the crate root). Example: *"roadmap P2.3 per-clock-domain registry"* →
   *"one `IOTA`/`DSCounter` field per clock domain (plan provenance: ondemand port P2.3)"*.
2. **Mechanical gate.** A checker (extending the existing `xtask` structure-check family) rejects
   codename patterns in `///`/`//!` text outside `Plan provenance:` lines and glossary sections.
   Validated by a rejecting mutation before it lands (phase methodology).
3. **Truth fixes.** Rewrite the `loop_graph.rs` header to describe its real (scalar + vector)
   role; delete the 11-warning dead cluster (or move what `pv_slice` genuinely needs into the
   diagnostic tree); ban file-level `#![allow(dead_code)]` in the crate via the same checker.
4. **Diagnostics quarantine.** Move `pv_slice` and `shadow` under `signal_fir/diagnostics/` with
   re-exports preserving the `compiler` test imports, and one header sentence each: *"observation
   only; never on the production path"*.
5. **Reading order.** Add a short *"How to read this crate"* section to `lib.rs` (order:
   `signal_prepare` → `clk_env` → `hgraph` → `schedule` → `module/` → `vector/mod.rs` stage map →
   one stage in `model → build → check` order), so the good existing headers become a guided tour.

### E2 — Recipe decomposition of the oversized functions (targets L1)

Apply the June method one level down: each function in the §1.3 table becomes a short
**orchestrator that reads as a table of contents** — a linear sequence of named phase functions,
each with a contract header (inputs, outputs, invariant maintained). Pure extract-function
refactoring; no reordering, no data-structure change.

- **Order of attack:** checker-side first (`verify_vector_plan`, `verify_prepared_signal`,
  `verify_fused_serial_groups_after_plan` — safest: a checker mistake fails closed and the golden
  corpus catches acceptance changes), then plan builders, then the lowerers, with `build_module`
  last (largest and most central).
- **Parameter bundles.** Where extraction would create ≥5-argument helpers, bundle the threaded
  mutable state first: in the vector lowerer the `(scope, cache, active)` triple becomes one
  `LowerCursor<'_>` struct — same borrow shape, one name. Mirror the June sub-state table by
  grouping `PureVectorLowerer`'s ~40 flat fields into the same kind of typed sub-states already
  documented at `module/mod.rs:330` (tables, sub-modules, external-control snapshots, UI).
- **Mechanical gate.** A ratcheting max-function-length check: starts at the current worst (767),
  every landing lowers the ratchet, ends at 200 for non-test functions. Same rejecting-mutation
  validation as E1.2.
- **Target:** 21 functions > 200 lines → 0; 69 > 100 → fewer than 30.

### E3 — One leaf grammar, two schedulers (targets L2)

Extract the **stateless** emission families shared by both production lowerers into one module
(`signal_fir/leaf_emit.rs`): numeric constants, binops (typing policy included), unary and binary
math ops, casts, min/max/pow. Each function is context-free — `fn emit_binop(store, ty, op, lhs:
FirId, rhs: FirId) -> FirId` — and each dispatcher arm becomes one call. This finishes what
`map_binop` started, and repeats at signal level the pattern that already succeeded at backend
level (the C-family emitter core, `porting/`-documented, all 7 drifts closed).

- **Scope guard (in):** an arm family enters `leaf_emit` only after a diff proves both paths emit
  the identical FIR shape for identical operand ids *today*.
- **Scope guard (out):** anything touching state, placement, caching, regions, UI, tables,
  delays, recursion — and `select2` until proven identical.
- **Doctrine guard:** this shares *producer-side vocabulary* only (exactly like `FirBuilder`
  itself). Checkers keep re-deriving their own evidence; the §3.2 producer/checker boundary of
  `vector/mod.rs` is untouched.
- **Target:** the two dispatchers shrink to their genuinely path-specific arms; a stateless-arm
  fix can no longer land on one path only.

### Interactions between experiments

E1 is independent of both others. E2 and E3 touch the same two dispatcher files; the composition
rule is per-file serialization: **within a file, land the E2 extraction before the E3
substitution** (small arms make the E3 diffs readable). No other coupling exists — any subset of
the three can land, in any order across files.

## 4. What must NOT be simplified (guardrails)

These properties look like duplication or pedantry to a fresh reader but are load-bearing:

1. **Producer/checker duplication** (`vector/mod.rs` header): a checker never calls its producer,
   never reuses a producer cache, never accepts producer-derived expected results. This is the
   assurance boundary — deduplicating it would be a regression even at zero FIR diff.
2. **Deterministic ordering:** `BTreeMap`/key-sorted iteration wherever emission order is
   derived (the emission-determinism gate exists precisely because a `HashMap` "simplification"
   once reordered output).
3. **Fail-closed vector fallback** with stable `FRS-VEC-FALLBACK-*` codes, and the
   status/effective-mode distinction (`VectorPipelineStatus` vs `VectorEffectiveMode`).
4. **`#![deny(missing_docs)]`** and the R9 docs/layout gates.
5. **Scalar/vector bit-exactness** and C++ parity anchors (provenance headers citing
   `8eebea429`).

## 5. Migration protocol (shared by all three experiments)

Per landing (one commit = one nameable step):

1. clean rebuild — never trust cached `.ir`/target state (known false-green trap);
2. **golden-FIR byte diff** over the corpus (`dsp/` + `tests/impulse-tests` corpora) across the
   mode matrix touched by the step (scalar, `-vec`, `-ss 0..3`, `-ec`, `-os`, `-double`, both
   `--table-init` modes) — byte-identical or the step is a defect;
3. full `cargo test -p transform` + workspace suite;
4. impulse oracle (133/133 × 8 backends) — structural certification is not numeric proof;
5. certification suite: 98 certified / 0 error across the 16 modes must be unchanged;
6. `make bench` / `make compile-bench` spot check when a step touches a hot path (E3 mainly);
7. every **new checker/gate** (E1.2, E2's ratchet) is validated by a rejecting mutation before it
   lands.

Suggested campaign order: **E1 → E2 → E3**, each on its own branch, journal entry per landing.

## 6. Expected end state (measurable)

| Metric | Today | Target |
|---|---:|---:|
| non-test functions > 200 lines | 21 | 0 |
| non-test functions > 100 lines | 69 | < 30 |
| comment lines citing plan codenames outside provenance sections | ≈200 | 0 |
| file-level `#![allow(dead_code)]` | 1 | 0 |
| stale module-header claims (measured) | 1 | 0 (gated) |
| stateless leaf-emission implementations | 2 | 1 |
| observation-only modules distinguishable by path | no | yes (`diagnostics/`) |

Total LOC is expected to drop only modestly (≈1–2 k lines: dead cluster, deduplicated leaf arms).
That is intentional: the objective of these experiments is **reading time**, not line count —
the line-count already has its own report (`scripts/loc_report.py`), and the July cleanup showed
that moving lines is easy while making them tell their story in present tense is the part that
pays.
