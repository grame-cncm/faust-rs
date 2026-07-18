# Session Handoff

Date: 2026-07-16

## Repo State

- Branch: `ondemand-vec-fad-synthesis`
- HEAD: the commit containing this handoff.

Recent commits (most recent first):

- `this commit` Complete lockstep SIMD remediation gates
- `c7db2ee8` Align Wasm pure-drop regression expectation
- `09d8798b` Attribute mixed lockstep SIMD to its source region
- `6cd34879` Carry lockstep delay-one state in registers
- `a85da004` Compact lockstep event certificates
- `08b04a8f` Reject scalar fallback in lockstep SIMD gate
- `a1ceeb87` Plan lockstep SIMD remediation

## Working Tree

- Tracked changes: final plan, journal, and handoff updates belong to this
  commit.
- Many unrelated pre-existing untracked scratch files remain untouched. The
  untracked `rad_lti_recursive_multi_output1` corpus/golden files and
  `vector_lockstep_simd_quad1.dsp` are not part of this task.

## Current Goal

- Section 8 and the lockstep SIMD remediation plan are complete.

## What Changed This Session

- Added vector-plan schema v2 lockstep bundles, lane isomorphism witnesses,
  transport layout, and independent rejecting checks.
- Added automatic exact-instance detection under `-vec`, one scheduler unit,
  stable logical lane/state ownership, and sample-interleaved event ordering.
- Added one physical FIR sample loop per bundle while preserving lane IEEE
  operation order, contraction policy, and planar external I/O.
- Added accepted width-two/width-four corpus cases, a rejected near-isomorphic
  case, Rust goldens, bit-exact integration tests, and optimizer parity.
- Added three profitability-oriented recursive cases, including two where the
  bundle is only one DSP subgraph, plus a Clang optimized-LLVM SIMD gate.
- Added compact two-sample event evidence so default `-vs 32` remains certified.
- Added checked register-carried delay-one state and row-transposed lockstep FIR
  assembly; unsupported state shapes remain array-backed.
- Attributed native vector operations through Clang line tables to the exact
  generated lockstep source loop, excluding separate mixed-DSP loops.
- Corrected the stale Wasm pure-`Drop` test exposed by the final workspace run.

## Decisions / Constraints

- Lockstep is automatic under `-vec`; there is no new CLI flag.
- Logical loop and recursion-group ids remain stable. One bundle is one
  scheduler node and one physical sample loop.
- Unsupported shapes and existing fused-delay slices fail closed. External
  `compute` buffers and chunk transports remain planar.
- The C++ compiler has no corresponding pass; Rustdoc records the adapted
  source provenance and invariants instead of claiming a 1:1 source port.

## Validation Run

- `cargo test -p transform --lib` -> 367 passed.
- `cargo test -p compiler --test vector_mode` -> 27 passed.
- `cargo test -p xtask --all-targets` -> 35 passed.
- `cargo run -p xtask -- vector-interp-opt-check` -> 40 traces matched.
- `cargo run -p xtask -- golden-check` -> tracked Rust golden corpus passed; an
  unrelated untracked DSP without a golden was temporarily excluded and then
  restored unchanged.
- `cargo run -p xtask -- lockstep-simd-check` -> 14 lockstep-attributed
  four-wide LLVM FP operations for each complex case; module totals 14/30/22.
- `cargo build --release -p impulse-runner` -> release harness rebuilt.
- `make -B -j8 interp-vec0 interp-vec1 -C tests/impulse-tests` -> 92 expected
  DSPs passed for each variant; documented `subcontainer1` exclusion only.
- `cargo clippy --workspace --all-targets -- -D warnings` -> passed.
- `cargo test --workspace --all-targets` -> codegen passes after the Wasm fix,
  then stops at the unrelated `p3_shadow_mode` assertion described below.

## Open Issues / Blockers

- The repository-wide test gate currently stops in the unrelated existing
  `recursive_apf_compute_body_reflects_all_four_cpp_schedules` assertion. It
  observes three distinct scalar recursive APF C++ forms where the test expects
  four; lockstep does not participate in scalar scheduling.
- Wider recognition of recursive loops already owned by a fused-delay slice is
  a conservative future optimization, not a correctness gap.

## Next Steps

1. Resolve or update the unrelated recursive APF scheduling-distinctness
   assertion before claiming the complete workspace test gate is green.
2. Consider composing fused-delay slices into lane-level lockstep units only
   with an extended certificate and equivalent mutation tests.

## Useful Commands to Resume

- `cargo test -p compiler --test vector_mode lockstep -- --nocapture`
- `cargo run -p xtask -- vector-interp-opt-check`
- `cargo run -p xtask -- lockstep-simd-check`
- `make -j8 interp-vec0 interp-vec1 -C tests/impulse-tests`
- `cargo run -p xtask -- golden-check`
