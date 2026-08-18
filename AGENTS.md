# AGENTS

Guidelines for contributors and coding agents working on `faust-rs`.

## 1. Project Goal

- Port the Faust compiler from C++ to Rust.
- Keep semantic parity with the C++ compiler as the default objective.
- Prefer explicit, testable behavior over speculative refactors.
- C++ reference branch used for baseline analysis: `master-dev-ocpp-od-fir-2-FIR19` (`8eebea429`) in /Users/letz/Developpements/RUST/faust folder.

## 2. Workspace Rules

- This repository is a Cargo workspace with many crates under `crates/`.
- Respect crate boundaries; avoid circular dependencies.
- Add new code in the most specific crate first, then expose upward through public APIs.
- Keep `crates/compiler` as the top-level orchestration crate (lib + CLI entry point).
- Use `clap` as the default command-line argument parser for user-facing binaries; use another parser only with an explicit documented reason in `porting/` or `JOURNAL.md`. This is enforced mechanically by `cargo run -p xtask -- cli-parser-check`, which flags any file with raw `env::args()`/`env::args_os()` that isn't an approved normalized Clap entry point or a documented exception in `porting/cli-parser-consolidation-analysis-and-porting-plan-2026-07-28-en.md` — every new `[[bin]]` must pass it, not just compile.
- Keep crate responsibilities aligned with `porting/faust-rust-porting-plan-en.md` section 2/4.
- Preserve key integrations recommended by the plan:
  - `patternmatcher` logic merged into `eval`.
  - `extended` math nodes integrated into `signals`.
  - `parallelize` integrated into `transform`.

## 3. Code Quality

- Rust edition and toolchain are controlled by workspace files.
- Before committing, run:
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace --all-targets`
  - the five static structure gates CI runs as its "Structure check" step —
    pure text/metadata analysis with no OS-dependent behavior, so any
    machine settles all three CI runners at once (the artifacts
    `code-graphs --check` compares hold crate and item names only, never
    paths, so they are byte-identical on every platform):
    - `cargo run -p xtask -- cli-parser-check`
    - `cargo run -p xtask -- error-model-check`
    - `cargo run -p xtask -- ffi-boundary-check`
    - `cargo run -p xtask -- structure-check`
    - `cargo run -p xtask -- code-graphs --check`
- Avoid introducing `unsafe` unless strictly required and documented.
- Tests must be self-contained: they must not depend on a locally installed
  Faust (e.g. `/usr/local/share/faust`), and copies of the Faust standard
  libraries must not be committed to the repository. When a test needs
  library-style DSP behavior, write a compact test-local Faust definition
  inline and compile it with the `compile_source_to_*` APIs (see
  `crates/compiler/tests/signal_fir_lane.rs` for the pattern).

## 4. CI Expectations

- CI runs on Linux, macOS, and Windows.
- CI stages include `cargo check`, formatting, clippy, and tests.
- CI also runs golden parity guardrails via `cargo run -p xtask -- golden-check`.
- CI also runs the compilation-cost gate via
  `cargo run --release -p xtask -- compile-budget-check`.
- CI also runs a Ubuntu-only "Structure check" step (see §3 for the five
  commands) enforcing the `clap`-only CLI policy, the error-model contract,
  the FFI-boundary/dependency-direction rule, file-size structure
  thresholds, and the checked-in code graphs plus the public-API baseline.
  It runs on one OS only because it is pure text/metadata analysis with no
  platform-dependent outcome — run it locally on any OS.
- `docs/code-graphs/public-api-baseline.txt` is the inventory of every crate's
  public items. When a change adds or removes one, regenerate with
  `cargo run -p xtask -- code-graphs` and review the diff in the commit: a new
  entry means a crate boundary widened, which is a design decision, not a
  side effect. Widening `pub(crate)` to `pub` merely to make a code move
  compile is the case this gate exists to surface.
- A change is not considered ready unless CI is green.
- Code that constructs, normalizes, displays, or compares filesystem paths must
  be checked for cross-platform behavior. Prefer `Path`/`PathBuf` operations,
  components, or explicit display-normalization helpers over ad hoc string
  concatenation, hardcoded separators, or Unix-only assumptions.
- For filesystem path assertions in tests, compare `Path`/`PathBuf` values (or
  components) instead of stringified paths; avoid hardcoded `/` separators
  because CI runs on Windows.
- In versioned documentation, generated reports, and stored test artifacts,
  prefer repository-relative paths over absolute local checkout paths so the
  content stays portable on GitHub and across contributor machines.

## 5. Porting Discipline

- Use the `porting/` documents as source of truth for scope and phases.
- Preserve behavior first, optimize later.
- Treat local quality gates as mandatory for each porting step:
  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace --all-targets`
  - `cargo run --release -p xtask -- compile-budget-check` before any
    commit that touches the compilation pipeline (`parser`, `eval`, `propagate`,
    `normalize`, `sigtype`, `transform`, `fir`, `codegen`, `compiler`) — see
    "Compilation-cost discipline" below.
- Add or update unit tests in the touched crate(s) as part of each porting change; if tests cannot be added immediately, record the reason, owner, and planned follow-up in `JOURNAL.md`.
- Document migrated source provenance as you port: add Rustdoc comments (`///` or `//!`) that reference the corresponding C++ source files/functions and capture key invariants/semantic notes needed to maintain parity.
- Public API migration is parity-driven, not blindly signature-driven:
  - internal Rust crate APIs may be adapted for idiomatic ownership/types/error handling,
  - external compatibility surfaces (CLI + C/C++ API tiers) target stable behavior and compatibility contracts.
- When a backend also exists in C++ Faust, the **generated code must expose the
  same public contract** as the C++ Faust output for that language, so existing
  architectures and projects keep working unchanged. Example: the Rust backend
  emits the host-supplied `F32`/`F64`/`FaustFloat` types, `ParamIndex`-based
  parameter access, and the `FaustDsp` trait expected by
  `faust2jackrust -source` / `faust2portaudiorust -source` projects (contract
  documented in the `crates/codegen/src/backends/rust/mod.rs` module header).
  Validate contract-affecting emitter changes by building generated output
  inside such projects.
- For each touched public API, document mapping status (`1:1`, `adapted`, or `deferred`) with rationale and compatibility impact in the relevant `porting/` phase document or `JOURNAL.md`.
- For representation-level adaptations (`adapted`) versus C++ data layout:
  - keep semantically coupled data co-localized with the owning node/instruction by default (avoid index-based side tables unless explicitly justified),
  - document invariants, potential failure modes, and mitigation tests in `porting/` or `JOURNAL.md`,
  - add at least one structural non-regression test for the adaptation itself.
- For tree-encoded IR crates, prefer canonical builder + matcher APIs over scattered helper ladders:
  - `boxes`: target `BoxBuilder` + `match_box`,
  - `signals`: target `SigBuilder` + `match_sig`.
  `boxes` no longer exposes public `box_*` / `is_box_*`; do not reintroduce them.
- Prefer real end-to-end integrations over temporary stubs; if a stub is unavoidable, it must be explicitly justified, owner-assigned, time-boxed, and removed within the same phase gate.
- Define explicit deliverables and pass criteria for each phase/prototype before implementation; do not start deep work on tasks with implicit success conditions.
- For critical compiler behavior, prefer differential tests against C++ reference outputs.
- For optimization-sensitive runtime paths (notably interpreter/backend execution),
  include a parity check between unoptimized and optimized execution
  (`opt_level=0` vs `opt_level=max`) on a representative subset to detect
  optimization-induced semantic drift.
- Assurance is tiered. Standard testing (unit + differential-vs-C++ + golden
  parity) is the default level for all porting work. Reserve the heavier
  producer/checker methodology described in
  `porting/lean-rust-certified-porting-plan-2026-07-11-en.md` — where a phase
  emits a canonical certificate that a small independent checker (in Rust,
  cross-checked by the Lean specification) must accept before the next phase runs
  — for phases whose output is a finite structural artifact consumed downstream
  and whose errors would be silent, such as scheduling, vector planning, and FIR
  routing. Do not apply it to ordinary steps, and never describe a lower assurance
  level as a proof of a higher one.
- Document known gaps and temporary scaffolding in `JOURNAL.md`.
- Follow the canonical pipeline described in the plan:
  - `parse -> boxes -> eval -> propagate -> normalize -> type/interval -> transform -> fir -> backend`
- New backends must preserve the Faust C++ lifecycle contract documented in
  `porting/backend-lifecycle-contract-en.md`. Before a backend is added to
  impulse, golden, or parity gates, it must include a backend-specific lifecycle
  conformance test proving:
  - `init = classInit -> instanceInit`;
  - `instanceInit = instanceConstants -> instanceResetUserInterface -> instanceClear`;
  - `instanceInit` does not call `classInit`;
  - runtime code does not duplicate `instanceClear` with ad-hoc field clearing;
  - compiled `instanceConstants` is authoritative when present.

### Compatibility Difference Registry (Mandatory)

- Keep `porting/faust-rs-vs-faust-cpp-differences-en.md` as the living registry
  of compatibility-relevant differences from the pinned Faust C++ reference.
- Update it in the same change whenever work introduces, changes, or removes:
  - a Rust-only source form, CLI option, backend, diagnostic, API, or artifact;
  - an intentional difference in defaults, accepted inputs, lifecycle,
    scheduling, generated output, failure policy, or observable behavior;
  - an `adapted`, `deferred`, narrower, or excluded public compatibility
    surface.
- Each entry must state compatibility impact, distinguish implemented behavior
  from plans, and link an authoritative plan, test, fixture, or API matrix.
- Closing a difference requires updating or removing its registry entry; a
  stale registry is a porting defect.

## 6. Scope and Non-Goals (Frozen)

- Keep these exclusions unless explicitly revised in planning docs:
  - `backend-java` is out of Rust port target scope.
  - legacy `-lang ocpp` mode is out of Rust port target scope.
- Treat maintained but non-primary paths as secondary until parity is reached on the production flow.

## 7. Phase 0 Gate (Mandatory Before Deep Implementation)

Before substantial implementation in a subsystem, confirm Phase 0 validation items are addressed for that scope (`porting/phases/phase-0-validation-en.md`):

- Effective compile pipeline confirmation (production path first).
- Differential baseline corpus and acceptance rules.
- `gGlobal` decomposition plan for touched flow.
- TreeArena hash-consing performance validation.
- API lifecycle and ownership model clarity for exposed entry points.

Do not lock large architectural decisions before these checks.

## 8. Critical Technical Risks to Re-check

From `porting/faust-rust-points-critiques-en.md`, keep these risks visible when designing changes:

- Parser migration parity (`bison/flex` semantics vs Rust parser stack).
- TreeArena performance regressions.
- Choosing the wrong initial signal->FIR path.
- Hidden `gGlobal` coupling in active compile flow.
- LLVM backend constraints (versioning/JIT/platform).
- Compiler-to-Wasm constraints.
- C API surface prioritization and lifecycle consistency.

When touching one of these areas, add focused tests/benchmarks in the same PR.

## 9. Golden Output Workflow

- Golden corpus inputs live in `tests/corpus/`.
- Golden reference outputs live in:
  - `tests/golden/rust/` (CI default gate)
  - `tests/golden/cpp/` (long-run parity target)
- Metadata and reference pinning live in `tests/golden/METADATA.toml`.
- Use:
  - `cargo run -p xtask -- golden-check` to validate against Rust reference snapshots.
  - `cargo run -p xtask -- golden-check-cpp` to validate against C++ reference snapshots.
  - `cargo run -p xtask -- golden-gen-rust` only for local bootstrap/scaffold updates.
  - `FAUST_CPP_BIN=/path/to/faust cargo run -p xtask -- golden-gen-cpp` for true C++ reference refresh.
- Any golden refresh must be documented in `JOURNAL.md` and mention reference commit/flags in PR description.

## 9bis. Compilation-Cost Discipline

Compilation speed is a user-visible contract, and a regression in it is silent:
every test stays green while large programs become unusable. The 2026-07-30
diagnostics-provenance arc multiplied front-end cost by 4.5x over the corpus and
by up to 17x on individual DSPs; it reached `main` through a green CI and was
found by a user, not by a gate. Treat cost like behavior.

- The gate is `cargo run --release -p xtask -- compile-budget-check`.
  Release profile only; it refuses to run under `debug_assertions`.
- It enforces two independent baskets, versioned in
  `tests/compile-budget/release-baseline.json`:
  - `codegen_cases` — full file-to-C++ path, scalar and vector;
  - `frontend_cases` — the `--check` path only, over `tests/impulse-tests/dsp`
    entries.
- Both baskets are enforced in **calibration units**, not milliseconds. Units
  are the point. Absolute wall-clock ceilings must be loosened until they
  survive the slowest runner, at which stage they no longer catch a 2x
  regression — the codegen ceilings carried 4.7x to 638x of headroom before
  normalization, the same width that let the 2026-07-30 front-end regression
  through. Each measurement is divided by the cost of a calibration DSP measured
  in the same process, so machine speed cancels out and the tolerance is 25%.
  The absolute ceilings and the vector/scalar ratio are retained as a coarse
  backstop only.
- The gate detects a per-case regression of 25% or more. Smaller ones pass, by
  design: measured run-to-run spread is ±8%, and a tighter bound would be flaky.
  Do not read a green gate as "no slowdown"; read it as "no slowdown this gate
  can distinguish from noise".
- Run it before committing any change to `parser`, `eval`, `propagate`,
  `normalize`, `sigtype`, `transform`, `fir`, `codegen`, or `compiler`. It costs
  a few minutes and is the only thing standing between a complexity defect and
  `main`.
- A failure is a finding, not an obstacle. Do not raise the baseline to make it
  pass. `--update` rewrites the recorded units, and **every increase must be
  justified in the commit message**; an unexplained `--update` is a regression
  ratified rather than fixed.
- Adding a case is always welcome; removing one from `REQUIRED_*_CASES` is
  rejected by the tool, because a silently shrinking basket reopens a known hole.
- Outstanding debt and the correction plan live in
  `porting/compile-time-provenance-regression-analysis-and-plan-2026-07-30-en.md`.
  Lower the baseline as each step lands.

## 10. Recursion and RouteIR Guidance

From `porting/faust-rust-recursion-model-note-en.md`:

- Keep `sigRec/sigProj` as canonical external signal form for parity/API stability.
- RouteIR-style recursion groups are acceptable as internal optimization/analysis IR.
- Maintain conversion boundaries and invariants:
  - explicit recursion boundaries,
  - arity correctness,
  - deterministic ordering,
  - semantic parity against legacy output.

## 11. Commit and Documentation Hygiene

- Keep the Git history **linear**: no merge commits. When a branch falls
  behind `main`, update it with `git rebase` (never `git merge`), so it is
  always possible to step back through history cleanly.
- Pull requests must be submitted in rebase form: a linear series of commits
  on top of the current `main`.
- Make small, coherent commits.
- Update `README.md` when user-facing build/run instructions change.
- Update `JOURNAL.md` for notable architecture, CI, or process changes.
- Keep comments and docs concise, factual, and implementation-oriented.

### Journaling Format (Daily Split)

- `JOURNAL.md` is now a **top-level index** (not the full monolithic journal body).
- Detailed journal content lives in `porting/journal/` with one file per day:
  - `porting/journal/YYYY-MM-DD.md`
- `porting/journal/README.md` lists the day files in chronological order
  (oldest day first).
- Inside each daily file, entries must be ordered by **Git commit recency**
  (most recent commit at the top, oldest at the bottom).
- Each journal entry should include:
  - `Commit date: YYYY-MM-DD`
  - `Source commit: <hash>` (or equivalent auditable commit reference) when
    generated from Git history.
- When reorganizing/splitting journal content, use the Git history of
  `JOURNAL.md` as the source of truth for ordering/dating instead of manual
  visual ordering.
- Preserve original journal **day buckets** (semantic day grouping) when
  generating or regenerating `porting/journal/*.md`.
- Do not reintroduce a large monolithic chronological body into `JOURNAL.md`;
  keep it as an index/redirect to the daily files.

### Session Handoff (Recommended)

- When ending a substantial work session (especially multi-step porting/backends),
  create or update `porting/HANDOFF.md` using `porting/HANDOFF_TEMPLATE.md`.
- Treat the handoff as a resumability artifact:
  - current branch/HEAD,
  - working tree state (tracked + notable untracked local files),
  - decisions taken,
  - validations run,
  - next steps and useful commands.
- Keep the handoff concise but concrete; prefer exact file paths and commands.
- If a session introduces major context shifts, update the handoff before the
  final commit (or explicitly document why it was skipped).

## 12. Collaboration Requirement During Porting (Mandatory)

- During implementation/porting work, if you encounter an ambiguity, missing
  requirement, or design tradeoff that is not already resolved by the active
  `porting/` documents or explicit user instructions, ask the user immediately
  before proceeding with the affected part.
- Do not silently choose behavior in parity-sensitive areas when requirements
  are unclear.
- This is especially mandatory for:
  - external C/C++ API compatibility decisions,
  - lifecycle/cache semantics,
  - ABI/layout/calling-convention choices,
  - UI/meta callback behavior,
  - unsupported-feature/error-policy decisions.
- When stopping to ask, state the concrete decision point and the impact on
  parity/compatibility so the user can answer quickly.
