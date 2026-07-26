# `crates/compiler` cleanup plan — 2026-07-26

Goal: make `crates/compiler` legible to a newcomer. A reader should be able to
follow one compilation from argv to emitted artifact, guess where a
responsibility lives, and trust the comments. Readability wins over brevity: an
abstraction that saves twenty lines but forces a reader to hop between three
files is a regression.

Behavior is frozen. Generated code, CLI output, diagnostic text and error codes
stay identical; any exception is deliberate, documented and covered by a test.
Public API renames are allowed where they make the surface regular; defects
found on the way are fixed with a rejecting test each.

## 1. Measured state

Production files, `crates/compiler` (2026-07-26):

| file | lines | note |
|------|-------|------|
| `src/lib.rs` | 2 729 | above the repo's 2 000-line review threshold |
| `src/cli/runner.rs` | 1 953 | **`run_main()` alone is 1 356 lines** |
| `src/cli/tests.rs` | 1 415 | test file, excluded from the threshold |
| `src/tests.rs` | 965 | test file, excluded |
| `src/signal_lowering.rs` | 759 | |
| `src/diagnostics.rs` | 576 | |
| `src/enrobage.rs` | 574 | |
| `src/execution.rs` | 506 | |
| `src/cli/args.rs` | 484 | |
| `src/json_naming.rs` | 446 | |
| `src/cli/diagnostics.rs` | 443 | |
| `src/error_mapping.rs` | 345 | |
| others | < 300 each | |

Three facts drive this plan.

**F1 — `run_main()` is 1 356 lines and contains two parallel dispatch ladders.**
Its structure is: argument validation (~220 lines of sequential `if … { usage
error }`), then the FIR-fixture mode (~400 lines, ~10 per-backend branches),
then the DSP-source mode (~730 lines, ~12 per-backend branches). The two
ladders answer the same question — "which backend did the user ask for?" — from
two different starting points (a `.fir` fixture already lowered, versus a `.dsp`
path to compile). Every branch repeats the same shape: build options, call the
emitter, `match` the result, print or `eprintln!` + `exit(1)`, and optionally
emit the JSON companion. Measured repetition across the file: 21 occurrences of
the `cli.import_dir.is_empty()` → `compile_file_*` / `compile_file_default_*`
choice, and 10 of the JSON-companion block.

**F2 — 48 public items are undocumented, all in one place.** `cargo rustdoc -p
compiler --lib -- -D missing-docs` reports 48 errors, every one a struct field
of a `CompilerError` variant (`lib.rs` 2450–2543). The sibling crate `transform`
already enforces this exact gate; `compiler` does not.

**F3 — `crates/compiler` is outside the structural guard.** `xtask
structure-check` enforces `MAX_PRODUCTION_LINES = 2_000`, but
`structure_check.rs` walks `crates/transform/src` only. Nothing prevents
`compiler` from drifting back after this cleanup.

Pre-existing and out of scope: `structure-check` currently fails on
`crates/transform/src/signal_fir/vector/lower/signal.rs` (2 100 lines), recorded
in the 2026-07-24 journal. Not a regression from this work, not to be "fixed"
in passing.

## 2. Phases

Each phase is one commit, green on its own (`cargo check --workspace
--all-targets` plus the crate's tests), with its own English journal entry.

### P1 — Document the `CompilerError` fields, then lock the gate

Document the 48 fields, then make `-D missing-docs` a standing requirement for
`compiler` the way it already is for `transform`. The fields are highly regular
(`source`, `error`, `diagnostics`), so the documentation must earn its place:
say what `source` actually holds (a display path or a logical source name,
depending on the entry point), and what invariant ties `error` to
`diagnostics` — not "the error" and "the diagnostics".

Risk: none (comments only). Verification: the gate command exits 0.

### P2 — Split `run_main()` by mode

Extract, in order and as pure moves:

- `validate_cli_arguments(&cli) -> ValidatedModes` — the ~220-line prologue,
  which today mixes mutually exclusive concerns (mode counting, empty-string
  flags, lane compatibility, architecture-path checks);
- `run_fir_fixture_mode(...)` — the `.fir` fixture ladder;
- `run_source_mode(...)` — the `.dsp` source ladder.

`run_main()` becomes a readable table of contents. Each extracted function moves
to its own module under `src/cli/` so no file returns to four digits.

Risk: this touches the binary's only entry path, and a mis-ordered validation
check changes which error a bad command line reports. Verification: a CLI
transcript differential (below) over every mode, plus the existing
`cli/tests.rs` suite.

### P3 — Factor the per-backend emit shape

With the two ladders isolated, the repeated shape becomes visible and can be
named once: "emit this artifact, then optionally its JSON companion, and exit
with this message on failure". One helper per ladder if their failure text must
differ (the fixture ladder says "fixture codegen failed", the source ladder
"pipeline failed"); one shared helper if it need not.

This phase must not merge branches that only look alike: backends differ in
option construction (`JuliaOptions` vs `RustOptions` vs `WasmOptions`) and some
produce binary output. Only the tail — emit, companion, failure — is shared.

Risk: silently changing an error message or an exit code. Verification: the same
transcript differential, which captures stderr and exit status.

### P4 — Split `lib.rs` into modules

`lib.rs` is already grouped by backend with banner sections and a convention
note (`fbe0f0b3`). Turn those sections into modules — one per backend family,
one for the helper service surface, one for `CompilerError` — keeping the
grouping and the note. The `Compiler` type and its builders stay in `lib.rs`.

Risk: a `pub use` slip silently changes the crate's public surface.
Verification: the exported-symbol list must be identical before and after
(`cargo public-api` if available, otherwise a sorted dump of `pub` items), and
every non-comment line must be byte-identical to its pre-move revision.

### P5 — Extend `structure-check` to `crates/compiler`

Make the guard walk `crates/compiler/src` too, so F1/F3 cannot silently return.
Only worth doing once P2 and P4 have brought every file under the threshold;
otherwise the check lands red and gets ignored.

## 3. Verification harness

Behavioral freeze is checked by a CLI transcript differential, not by eye: for a
fixed set of DSP inputs and every mode (`--lang` × {cpp, c, rust, julia, asc,
interp, cranelift, wasm, wast, fir}, plus `--dump-*`, `--check`, `--golden`,
`--svg`, `--json`), capture stdout, stderr and exit status from the
pre-phase binary, then from the post-phase binary, and diff. Two known-unstable
fields are normalized: the Cranelift `compute_entry_addr` (a runtime address)
and any temporary path.

This is the harness that would have caught the `resolve_module_name(None, …)`
regression during the Cranelift work — a silent module rename that no type check
could see.

Per-commit gates:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --all-targets
cargo test -p compiler
cargo rustdoc -p compiler --lib -- -D missing-docs   # from P1 on
cargo run -p xtask -- golden-check
```

## 4. Explicitly out of scope

- `enrobage.rs`, `diagnostics.rs`, `execution.rs`, `paths.rs`: under the
  threshold and internally coherent. Documentation fixes only if something is
  found false.
- The `transform` crate, including its failing `structure-check` finding.
- Test files (`tests.rs`, `cli/tests.rs`, `tests/`): they are excluded from the
  line threshold and their size reflects coverage, not disorder. Touched only
  where a phase requires it.
- Any change to emitted code, CLI output, diagnostics or error codes.
