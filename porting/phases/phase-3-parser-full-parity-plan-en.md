# Phase 3 Parser Full Parity Plan (Rust vs C++)

## 1. Goal

Reach parser parity that is both:
- functionally complete (lexer + grammar + semantic actions + diagnostics + imports),
- continuously verified against the C++ reference implementation.

Reference C++ sources:
- `/Users/letz/Developpements/RUST/faust/compiler/parser/faustparser.y`
- `/Users/letz/Developpements/RUST/faust/compiler/parser/faustlexer.l`

## 2. Current Baseline (as of 2026-02-19)

- `crates/parser` is a production facade delegating to `parser-proto`.
- Parser-proto has broad slice coverage and local tests are green.
- Coverage reports show no unresolved token/state/nonterminal gaps after alias mapping.
- Full parity is not yet closed:
  - remaining-step checklist in `phase-3-parser-en.md` is still open,
  - semantic action mapping still tracks open items for 100% parity,
  - parser-adjacent modules (`sourcefetcher`, `enrobage`) are explicitly deferred,
  - strict differential run against C++ reveals at least one mismatch (`err_01_parse_missing_rhs.dsp`).

Follow-up (2026-08-11): the historical Phase 3 deferral remains true of the
current implementation, but `sourcefetcher` now has an approved adapted design
and executable Phase 9 plan in
[`sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md`](../sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md).

## 3. Execution Plan

### Step 1: Stabilize Differential Baseline

- Scope:
  - harden `crates/parser-proto/tests/cpp_differential.rs` so parser-valid and phase-later failures are classified consistently.
  - keep parser mismatch assertions focused on parser acceptance/recovery semantics.
- Deliverables:
  - deterministic differential policy documented in test comments + `porting/`.
  - no false mismatch for known non-parser failures.
- Pass criteria:
  - `FAUST_CPP_BIN=/usr/local/bin/faust cargo test -p parser-proto --test cpp_differential -- --nocapture` passes.

### Step 2: Remove Prototype Recovery Paths

- Scope:
  - remove `LexProbeToken` fallback/recovery from `crates/parser-proto/src/grammar/faustparser.y`.
  - replace prototype unsupported-token branches with fully migrated grammar rules.
- Deliverables:
  - grammar no longer depends on prototype token catch-all.
- Pass criteria:
  - parser grammar builds warning-clean in strict mode.
  - no regression on parser slice tests and diagnostics suite.

### Step 3: Close Grammar + Semantic Action Parity

- Scope:
  - close all remaining semantic-action parity deltas against C++ behavior.
  - verify action formulas on structure (Tree/Box shape), not pointer identity.
- Deliverables:
  - updated mapping status in `phase-3-semantic-action-mapping-en.md` with no open action families.
- Pass criteria:
  - `cargo test -p parser-proto --test parser_semantic_parity`.
  - open-items section reduced to zero remaining action families.

### Step 4: Close Diagnostics and Recovery Parity

- Scope:
  - align malformed-input classes and structured diagnostics (file/line/column/range + parser code family).
  - align recovery behavior on malformed statement boundaries.
- Deliverables:
  - expanded malformed corpus + expected envelopes documented.
- Pass criteria:
  - `cargo test -p parser-proto --test parser_diagnostics`.
  - no untriaged Rust/C++ class mismatch on malformed fixtures.

### Step 5: Complete Import/SourceReader Parity Envelope

- Scope:
  - validate import behavior on larger import graphs and stdlib-heavy fixtures.
  - migrate the production import path toward structural C++ parity:
    preserve import nodes through parse and expand them from the parsed
    definition tree instead of from raw source text.
  - keep explicit status for `sourcefetcher`/`enrobage` (implemented or deferred by design with rationale).
- Deliverables:
  - updated `phase-3-parser-adjacent-modules-status-en.md`.
  - implementation progress tracked against
    `porting/parser-import-structural-cpp-parity-plan-2026-03-29-en.md`.
- Pass criteria:
  - import cycle/unresolved/import-graph tests pass deterministically.
  - inline and multiline import placement are semantically equivalent.
  - documented lifecycle status (`1:1`, `adapted`, or `deferred`) for adjacent modules.

### Step 6: Expand Differential Validation to Full Corpus

- Scope:
  - run parser differential on:
    - full `tests/corpus/rep_*.dsp`,
    - malformed suite,
    - stdlib/import-heavy set.
- Deliverables:
  - refreshed `phase-3-parser-parity-report-en.md` + mismatch triage table.
- Pass criteria:
  - zero untriaged parser-class mismatches.
  - every residual delta has owner + rationale + follow-up date.

### Step 7: Promote Final Implementation to Production Parser

- Scope:
  - replace bridge-only posture by final parser integration path in `crates/parser`.
  - preserve stable parser public API expected by `compiler`.
- Deliverables:
  - production parser path no longer treated as prototype delegation.
- Pass criteria:
  - `crates/compiler` parse and dump-box flows consume production parser path end-to-end.

## 4. Verification Matrix

Required local gate before each parity commit:
- `cargo fmt --all`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test -p parser-proto --no-fail-fast`
- `cargo test -p parser --no-fail-fast`
- `FAUST_CPP_BIN=/usr/local/bin/faust cargo test -p parser-proto --test cpp_differential -- --nocapture`
- `cargo run -p xtask -- parser-parity-report`

## 5. Definition of Done

All conditions must hold:
- parser-phase remaining checklist closed in `phase-3-parser-en.md`,
- no prototype fallback paths in grammar,
- parser differential (Rust vs C++) passes on full target corpus without untriaged mismatch,
- parser diagnostics parity envelope validated on malformed corpus,
- production parser integration closed (no prototype-only dependency path).
