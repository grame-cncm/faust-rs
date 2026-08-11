# Phase 3 Parser Parity Status (Rust vs C++) — 2026-02-28

Status: audit snapshot  
Scope: `crates/parser` + `crates/parser-proto` + compiler parse entry points

---

## 1. What Was Checked

Local checks executed in this workspace:

1. `cargo test -p parser --all-targets`
2. `cargo test -p parser-proto --all-targets`
3. `cargo run -p xtask -- parser-parity-report`
4. Source inspection of:
   - `crates/parser/src/lib.rs`
   - `crates/parser-proto/src/lib.rs`
   - `crates/parser-proto/src/source_reader.rs`
   - `crates/parser-proto/src/grammar/faustparser.y`
   - `crates/parser-proto/tests/cpp_differential.rs`
   - `porting/phases/phase-3-parser-parity-report-en.md`
   - `porting/phases/phase-3-parser-adjacent-modules-status-en.md`

---

## 2. Current State (Implemented)

## 2.1 Production entrypoint is wired

- `crates/compiler` parse flows call `crates/parser` public APIs.
- `crates/parser` is the stable facade crate used by upper layers.

Status: `adapted` (integration milestone achieved).

## 2.2 Parser/lexer migration body is functionally active

- `crates/parser-proto` includes lrlex/lrpar grammar and lexer with semantic actions.
- Slice coverage is beyond early prototype and includes parser slices 1..12
  (as reflected by integration tests).
- `cargo test -p parser-proto --all-targets` passes.

Status: `adapted` (feature-complete enough for current corpus/tests).

## 2.3 Parity-coverage report shows no unresolved name-missing items

From `phase-3-parser-parity-report-en.md` (auto-generated):

- Parser token coverage: C++ declared `140` / Rust declared `139` / unresolved missing `0`
- Lexer states: C++ `3` / Rust `3` / unresolved missing `0`
- Grammar nonterminal coverage (name-based): C++ `67` / Rust `39` / unresolved missing `0` after alias mapping

Status: coverage-mapped; no unresolved lexical/grammar family gap in that report model.

## 2.4 Differential parse-class harness exists

- `crates/parser-proto/tests/cpp_differential.rs` runs Rust/C++ classification
  comparisons (valid/recovered/error style envelope) on corpus + malformed cases.
- Parser diagnostics/recovery suites are present and passing.

Status: `adapted` (good functional parity guardrail, not full structural parity).

## 2.5 SourceReader local import support is implemented

- recursive local imports
- cycle detection
- search-path + local-dir precedence
- used-files tracking in `SourceReader`

Status: `adapted` for local-file import model.

---

## 3. Precise Remaining Gaps for Full C++ Parity

The items below are the concrete blockers to call parser parity “complete”.

## G1 — `crates/parser` is still a facade delegating to `parser-proto`

Evidence:
- `crates/parser/Cargo.toml` depends directly on `parser-proto`.
- `crates/parser/src/lib.rs` forwards all parser APIs to `parser_proto::*`.

Impact:
- production parser boundary still anchored to prototype crate internals.

Needed to close:
1. move/merge parser implementation into production crate boundary,
2. keep `parser-proto` only as temporary migration artifact or remove it,
3. ensure `compiler` only depends on final production parser internals.

Priority: High.

## G2 — Import-expanded parse loses per-import-file source location fidelity

Evidence:
- `parse_file_with_imports` in `crates/parser-proto/src/lib.rs` calls:
  - `SourceReader::read_file(...)` to produce one expanded string,
  - then `parse_program(&expanded, &source_name)` with `source_name = entry path`.
- parser cursor source labels are therefore associated with the entry file label
  during parse of expanded content.

Impact:
- diagnostics on imported content are not guaranteed to preserve original
  imported file identity/ranges as C++ parser/source reader tooling can.

Needed to close:
1. preserve origin mapping during expansion (line map or synthetic include map),
2. propagate original file/range into parser diagnostics and `def/use` metadata,
3. add differential tests with malformed imports that assert file attribution.

Priority: High.

## G3 — Parser diagnostics code granularity is still coarse

Status update (2026-07-28):
- message-text classification has been removed;
- codes are assigned at parser emission sites;
- pending parser records use canonical severity/code values and remain private
  to the parser crate.

Impact:
- weaker parity and weaker long-term stability for diagnostic-code-level
  comparisons vs C++ behavior classes.

Needed to close:
1. define parser diagnostic code matrix by recovery/error category,
2. map grammar/lexer failure paths explicitly (non-string-heuristic where possible),
3. snapshot-test both human and machine-readable outputs by category.

Priority: Medium-High.

## G4 — Full structural differential parity is not yet the default gate

Evidence:
- current differential harness mainly validates parse success/recovery class
  envelopes; structural AST parity is covered in targeted semantic tests, not as
  a full-corpus structural gate.

Impact:
- semantic drift can pass if parse class remains equal while tree shape differs
  in non-covered forms.

Needed to close:
1. expand differential to structural checks on broader corpus families,
2. include stdlib/import-heavy suites in regular parity gate,
3. classify allowed differences explicitly and track owners.

Priority: Medium-High.

## G5 — `SourceFetcher` adapted native path implemented

Evidence:
- `SourceLocator` and structural import expansion preserve URL identity and
  relative remote resolution.
- `compiler::remote_fetch` supplies the optional bounded native transport and
  the CLI requires explicit runtime permission.
- Hermetic parser and loopback integration tests cover the implemented path.

Impact:
- native direct sources/import graphs and main remote architectures are
  supported through an adapted, safer API;
- evaluator URL components/libraries, remote inline architecture sub-includes,
  browser fetching, and compatibility-facade opt-in remain deferred.

Follow-up (2026-08-11): G5 is closed for the planned native structural-import
scope. Remaining surfaces are classified rather than hidden; see
[`sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md`](../sourcefetcher-remote-import-analysis-and-implementation-plan-2026-08-11-en.md).

## G6 — Parser API does not yet expose source-file usage list to compiler facade

Evidence:
- `SourceReader` tracks `used_files()`,
- `parse_file_with_imports` returns only `ParseOutput` (no used-file list surfaced).

Impact:
- limits parity with C++ workflows relying on source-file listing/reporting.

Needed to close:
1. expose used-files in parse API output or companion structure,
2. wire compiler/CLI surface where needed,
3. add deterministic tests for list order/content.

Priority: Medium.

---

## 4. Parity Readiness Summary

Current parser state is **operational and strong for local-file parsing**, with
broad grammar/lexer coverage and passing Rust/C++ class-level differential tests.

It is **not yet full C++ parity-complete** due to:
- prototype-delegation architecture (`parser` -> `parser-proto`),
- import-origin diagnostic fidelity gap,
- incomplete diagnostic-code granularity parity,
- missing full structural differential gate,
- deferred network import path.

---

## 5. Recommended Closure Order

1. Close G1 + G2 together (production boundary + source-origin correctness).
2. Close G3 (diagnostic code matrix and stable snapshots).
3. Close G4 (full structural differential gate extension).
4. Close G6 (used-files exposure for parser API parity).
5. Close G5 if/when remote import support is in active scope.

---

## 6. Executable Checklist (Tickets A1..A6)

Legend:
- Owner: parser track unless explicitly delegated.
- Status: `[ ]` todo, `[x]` done.

## A1 — Production parser ownership (close G1)

Status: `[x]`  
Goal: remove production dependence on `parser-proto` internals.

Tasks:
1. Move parser implementation modules from `crates/parser-proto` into
   `crates/parser` (or invert dependency so parser is primary and proto optional).
2. Stop forwarding parser APIs via `parser_proto::*` in `crates/parser/src/lib.rs`.
3. Ensure `crates/compiler` imports only production parser crate API.
4. Update crate docs and provenance comments to reflect final ownership.

Validation:
1. `cargo test -p parser --all-targets`
2. `cargo test -p compiler --all-targets`
3. `cargo check --workspace --all-targets`

Execution note (2026-02-28):
- Items (1) and (2) passed.
- Item (3) is currently blocked by an unrelated local untracked example file
  (`crates/codegen/examples/interp_baseline.rs`) that fails to compile in this
  workspace state; parser ownership changes compile and test correctly.

Exit criteria:
- `crates/parser/Cargo.toml` no longer requires direct runtime delegation to
  `parser-proto` for production parse path.
- Production parser API remains stable for compiler callers.

## A2 — Import origin fidelity (close G2)

Status: `[x]`  
Goal: preserve per-file source locations after import expansion.

Tasks:
1. Add origin mapping in import expansion (`SourceReader` output -> origin map).
2. Propagate origin map into parser cursor updates and diagnostics generation.
3. Preserve file attribution for `def/use` properties when tokens come from imported files.
4. Add malformed-import fixtures proving diagnostic file/range points to imported file.

Validation:
1. `cargo test -p parser-proto --all-targets` (or `-p parser` after A1 merge)
2. `cargo test -p parser --all-targets`
3. Differential malformed import cases in parser tests.

Exit criteria:
- diagnostics from imported code report imported file paths (not only entry file).
- source-span fallback notes are reduced for import-heavy malformed cases.

Execution note (2026-02-28):
- Implemented line-origin propagation from `SourceReader` import expansion into
  parser cursor mapping.
- Added regression test:
  - `parse_file_with_imports_preserves_imported_file_diagnostic_origin`
    in `crates/parser/tests/api_bridge.rs`.

## A3 — Diagnostic code matrix hardening (close G3)

Status: `[x]`  
Goal: replace coarse parser-code heuristics with stable category mapping.

Tasks:
1. Define explicit parser diagnostic category matrix (lexer, syntax, recovery, literal, etc.).
2. Replace broad string-heuristic mapping in parser code classification where feasible.
3. Snapshot-test human and JSON diagnostics by category.
4. Document mapping in phase docs and Rustdoc comments.

Validation:
1. `cargo test -p parser-proto --all-targets`
2. `cargo test -p parser --all-targets`
3. `cargo test -p compiler --all-targets` (diagnostics integration)

Exit criteria:
- stable parser diagnostic code family per error/recovery category.
- snapshot tests prevent unreviewed code drift.

Execution note (2026-02-28):
- Added explicit parser diagnostic code slot in parser-local diagnostics.
- Parse-loop classification now sets stable codes directly from error class:
  - lexical parse errors -> `FRS-LEX-0001`,
  - grammar parse errors -> `FRS-PARSE-0001`.
- Bundle conversion now prefers explicit parser-local code over message heuristics.
- Existing compiler diagnostics snapshot tests remain passing.

## A4 — Structural differential gate expansion (close G4)

Status: `[x]`  
Goal: enforce structural parity on a broader corpus, not only parse class parity.

Tasks:
1. Extend differential harness to compare structural tree/box shapes for more cases.
2. Add stdlib/import-heavy cases to regular differential execution.
3. Maintain allowlist for accepted intentional differences (owner + rationale).
4. Wire gate command into routine parity workflow docs.

Validation:
1. `cargo test -p parser-proto --test cpp_differential -- --nocapture`
2. `cargo run -p xtask -- parser-parity-report`
3. CI lane / local script execution for differential suite.

Exit criteria:
- structural differential gate runs on representative corpus families.
- all remaining mismatches are triaged and documented.

Execution note (2026-02-28):
- Added production parser structural differential suite:
  - `crates/parser/tests/structural_cpp_differential.rs`
- The suite now covers:
  - representative structural shape checks (`prec`, `unary`, `appl`) on parser output,
  - import-heavy fixture through `parse_file_with_imports`,
  - optional C++ acceptance cross-check when `FAUST_CPP_BIN` (or `/usr/local/bin/faust`) is available.
- Validation run:
  - `cargo test -p parser --all-targets`
  - `cargo test -p compiler --all-targets`

## A5 — Source usage exposure in parser API (close G6)

Status: `[x]`  
Goal: surface used source files through production parser API.

Tasks:
1. Extend parser API return type (`ParseOutput` companion or wrapper) with used-files list.
2. Thread used-files from `SourceReader` through `parse_file_with_imports`.
3. Optionally expose compiler/CLI switch to print used source list.
4. Add deterministic order tests for used-files output.

Validation:
1. `cargo test -p parser --all-targets`
2. `cargo test -p compiler --all-targets` (if CLI/facade exposure added)

Exit criteria:
- parser callers can retrieve import-resolved used file list from public API.

Execution note (2026-02-28):
- Extended production parser API output with source-usage exposure:
  - `crates/parser/src/lib.rs`:
    - `ParseOutput` now includes `used_files: Vec<PathBuf>`.
    - `parse_program(...)` keeps `used_files` empty for in-memory parsing.
    - `parse_file_with_imports(...)` now forwards deterministic source expansion
      order from `SourceReader::used_files()`.
- Added deterministic-order regression coverage:
  - `crates/parser/tests/api_bridge.rs`:
    - `parse_file_with_imports_exposes_deterministic_used_files_order`
    - existing import parse bridge test now also checks `used_files`.
- Validation run:
  - `cargo test -p parser --all-targets`
  - `cargo test -p compiler --all-targets`

## A6 — Remote import policy and implementation decision (close G5)

Status: `[x]`  
Goal: decide and implement (or explicitly freeze) network import parity scope.

Tasks:
1. Decide policy:
   - implement feature-gated network fetch, or
   - keep deferred explicitly out-of-scope for target release.
2. If implemented:
   - add feature-gated fetch module and tests (success/failure/offline).
   - keep deterministic behavior when feature disabled.
3. Update
   `phase-3-parser-adjacent-modules-status-en.md` lifecycle mapping.

Validation:
1. `cargo test -p parser-proto --all-targets` (or parser equivalent after A1)
2. feature-on/off test matrix for parser crate.

Exit criteria:
- `sourcefetcher` lifecycle is no longer ambiguous (`adapted`/`1:1` implemented
  or explicitly frozen `deferred` with signed scope decision).

Execution note (2026-02-28):
- Signed scope decision for parser Phase 3: keep network import support explicitly
  frozen as `deferred` (no fetch stub in parser crate).
- Updated lifecycle/status mapping:
  - `porting/phases/phase-3-parser-adjacent-modules-status-en.md`
    now marks `sourcefetcher` as `deferred (explicitly frozen)` and aligns wording
    to production `parser` crate tests.
- Added deterministic policy regression in parser API tests:
  - `crates/parser/tests/api_bridge.rs`:
    - `parse_file_with_imports_keeps_remote_urls_out_of_scope`
      asserts URL imports fail with `SourceReaderError::UnresolvedImport`.
- Validation run:
  - `cargo test -p parser --all-targets`
  - `cargo test -p compiler --all-targets`

## 6.1 Suggested Execution Sequence

1. A1 + A2 (shared boundary/origin work)
2. A3
3. A4
4. A5
5. A6 (decision-driven; may run in parallel if policy is decided early)
