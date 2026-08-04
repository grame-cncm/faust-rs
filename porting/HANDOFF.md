# Session Handoff

Date: 2026-08-04

## Repo State

- Branch: `main-dev`
- Implementation base: `440c5466` (`Correct Cmajor impulse blocker analysis`).
- The current implementation commit adds shared precedence-aware textual
  expression layout and removes `bells` from the Cmajor exclusions.

Recent commits (most recent first):

- `440c5466` Correct Cmajor impulse blocker analysis
- `c9bfd9d3` Record Cmajor impulse handoff
- `274bc934` Integrate Cmajor impulse gate
- `74b5ef05` Add Cmajor impulse harness
- `35b6e755` Specify Cmajor impulse test lane
- `aa0fc0fa` Qualify scalar Cmajor backend
- `be817b06` Activate scalar Cmajor facade and CLI
- `b85d7d5f` Validate concrete Cmajor table lowering
- `ac2c5daa` Add Cmajor UI events and bargraphs
- `0ef0c895` Add scalar Cmajor emitter core
- `ccc3e5ee` Plan scalar Cmajor backend port

## Working Tree

- Tracked changes at handoff preparation: this handoff and its daily journal
  index/entry only.
- Numerous pre-existing untracked user DSPs, patches, reports, generated files,
  and impulse scratch directories remain untouched and uncommitted.

## Current Goal

- Complete C6 by adding direct runtime validation of Cmajor output events and
  bargraph cadence, while reducing the explicit impulse exclusions.

## What Changed This Session

- Implemented and documented a scalar canonical-FIR-to-Cmajor backend with
  facade, CLI, f32/f64, lifecycle, UI, bargraphs, state, tables, and typed
  errors.
- Added pinned-C++ structural differentials and Cmajor-generated-C++ runtime
  optimization parity.
- Added an opt-in, parallel-safe `tests/impulse-tests/Make.cmajor` lane and the
  top-level `make cmajor` target.
- Fixed comparison result typing, lifecycle-time bargraph endpoint writes, and
  repeated UI display-address handling as discovered by the impulse corpus.
- Added reusable precedence-aware textual expression layout and migrated
  Cmajor to it; `bells` now passes the external impulse lane.

## Decisions / Constraints

- Reference Faust C++ is
  `8eebea4294a44a5260484c750d332781ed9f8ffd`; tested Cmajor is 1.0.3175.
- Cmajor is an external scalar lane, intentionally outside default `make all`
  and the vector/scheduling matrix.
- Each DSP owns `build/cmajor/<dsp>/` so `make -j` is collision-free.
- The pinned C++ impulse architecture and `cmajor_cpp_dsp` adapter are referenced
  in place and are not copied into faust-rs.
- The adapter validates audio and input controls but does not drain bargraph
  output events; a separate runtime test is still required.

## Validation Run

- Direct `bells` Cmajor impulse target -> pass: frontend accepted the generated
  source and its runtime trace matched `reference/bells.ir`; maximum source
  parenthesis depth fell from 111 to 3.
- `make -f Make.cmajor all` -> pass/no pending targets; 126 supported Cmajor
  traces are present after removing `bells` from the exclusions.
- Cmajor backend unit tests and 17 compiler integration tests -> pass.
- `RUSTDOCFLAGS=-Dwarnings cargo doc -p codegen -p compiler --no-deps` -> pass.
- `cargo fmt --all -- --check` -> pass.
- `cargo clippy --workspace --all-targets -- -D warnings` -> pass.
- `cargo test --workspace --all-targets` -> pass.
- `cargo run -p xtask -- golden-check` -> pass.
- `cargo run --release -p xtask -- compile-budget-check` -> pass, no baseline
  update; final `bells` front-end measurement 13.800 units against an unchanged
  18.318-unit ceiling.

## Open Issues / Blockers

- Seven impulse exclusions are explicit in `tests/impulse-tests/known.mk`:
  shared `subcontainer1`; one-sample-incompatible `bs`; unsupported `sound`;
  and generated tables expanded into oversized literal initializers in
  `modulations`, `osci`, `tester`, and `tester2`.
- The upstream adapter prints lifecycle `checkDefaults` warnings for some DSPs
  because its reset methods are no-ops. Traces still match; direct generated
  lifecycle tests remain authoritative.
- `golden-check-cpp` still has the previously recorded 34 unrelated metadata
  name mismatches; no snapshot was changed.

## Next Steps

1. Add Cmajor runtime event capture for UI mutation and bargraph cadence.
2. Preserve table-generator provenance and emit compact `SIG0`/`fill..._<size>`
   code to remove `modulations`, `osci`, `tester`, and `tester2`.
3. Reassess soundfile support and the shared subcontainer gap separately.

## Useful Commands to Resume

- `make -C tests/impulse-tests -j4 cmajor CMAJ_BIN=/usr/local/bin/cmaj FAUST_CPP=/Users/letz/Developpements/RUST/faust/build/bin/faust`
- `make -C tests/impulse-tests -f Make.cmajor all dspfiles='dsp/APF.dsp' CMAJ_BIN=/usr/local/bin/cmaj`
- `CMAJ_BIN=/usr/local/bin/cmaj FAUST_CPP_BIN=/Users/letz/Developpements/RUST/faust/build/bin/faust cargo test -p compiler --test cmajor_backend`

## Notes

- The 126 passing traces include every current `upsampling_*` and
  `downsampling_*` impulse fixture.
