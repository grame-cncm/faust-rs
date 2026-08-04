# Session Handoff

Date: 2026-08-04

## Repo State

- Branch: `main-dev`
- Session base HEAD: `be817b064d3a47030122b62347d2be18e53144cf`
- The qualification changes described here are committed immediately after
  this handoff as the next linear commit.

Recent commits (most recent first):

- `be817b06` Activate scalar Cmajor facade and CLI
- `b85d7d5f` Validate concrete Cmajor table lowering
- `ac2c5daa` Add Cmajor UI events and bargraphs
- `0ef0c895` Add scalar Cmajor emitter core
- `ccc3e5ee` Plan scalar Cmajor backend port

## Working Tree

- Tracked changes at handoff preparation: Cmajor qualification tests and this
  session's plan, journal, README, and handoff updates.
- Numerous pre-existing untracked user DSPs, patches, reports, and generated
  files remain in the repository root; the Cmajor work did not modify or stage
  them.

## Current Goal

- Qualify the scalar `-lang cmajor` backend against pinned Faust C++ and the
  current Cmajor toolchain, then complete the remaining C6 runtime matrix.

## What Changed This Session

- Added a documented canonical-FIR-to-Cmajor scalar emitter with f32/f64,
  streams, lifecycle, state, delays, loops, math, UI events, 50 Hz bargraphs,
  and concrete read/write/waveform/generated tables.
- Added compiler facade, CLI, architecture wrapping, intrinsic one-sample and
  external-control capability routing, typed diagnostics, and public Rustdoc.
- Added frontend, pinned-C++ observable-contract, and Cmajor-generated-C++
  `-O0`/`-O4` recurrence tests.

## Decisions / Constraints

- Reference Faust C++ is commit
  `8eebea4294a44a5260484c750d332781ed9f8ffd`.
- Lifecycle is intentionally adapted to the repository-wide contract:
  `init = classInit -> instanceInit`, and direct `instanceInit` does not call
  `classInit`.
- The C++ constant-table optimizer difference is excluded from the narrow
  source differential; runtime semantics remain the required oracle.
- C7/C8 polyphonic, DSP-event, hybrid, and SDK application layers are deferred.

## Validation Run

- `cargo fmt --all -- --check` -> pass.
- `cargo clippy --workspace --all-targets -- -D warnings` -> pass.
- `cargo test --workspace --all-targets` -> pass.
- `cargo run -p xtask -- golden-check` -> pass.
- `cargo run --release -p xtask -- compile-budget-check` -> pass, no baseline
  update.
- `RUSTDOCFLAGS=-Dwarnings cargo doc -p codegen -p compiler --no-deps` -> pass.
- `CMAJ_BIN=/usr/local/bin/cmaj FAUST_CPP_BIN=... cargo test -p compiler
  --test cmajor_backend` -> 17 pass, including C++ differential and Cmajor
  runtime at `-O0`/`-O4`.
- `cargo run -p xtask -- golden-check-cpp` -> 34 known unrelated failures:
  stored filename-derived metadata names differ from current `mydsp`; no Cmajor
  snapshot or baseline changed.

## Open Issues / Blockers

- C6 still needs runtime event delivery, bargraph cadence, tables, broader
  f32/f64 numeric/impulse coverage, and explicit negative Cmajor mutations.
- The full C++ golden gate is not green because of the unrelated metadata-name
  baseline drift described above.

## Next Steps

1. Extend the Cmajor-generated-C++ harness with input streams and event I/O.
2. Cover UI mutation, bargraph cadence, table values, and f64 runtime behavior.
3. Add the compact interpreter-versus-Cmajor impulse matrix and only then mark
   C6 and the scalar completion checklist green.

## Useful Commands to Resume

- `CMAJ_BIN=/usr/local/bin/cmaj FAUST_CPP_BIN=/Users/letz/Developpements/RUST/faust/build/bin/faust cargo test -p compiler --test cmajor_backend`
- `cargo test --workspace --all-targets`
- `cargo run -p xtask -- golden-check`
- `cargo run --release -p xtask -- compile-budget-check`

## Notes

- Optional tools are never auto-discovered in normal tests; their explicit
  environment variables keep CI self-contained.
