# Session Handoff

Date: 2026-08-13

## Repo State

- Branch: `main-dev`
- Implementation HEAD at handoff preparation: `6c15283b0a0b694344c0ed0c0184e20c67d06c5a`
  (`Harden mem0 differential and optimization gates`); the M9 documentation
  closeout commit contains this handoff.

Recent implementation commits (most recent first):

- `6c15283b` Harden mem0 differential and optimization gates
- `b8b883b7` Integrate audited mem0 impulse tests
- `14d64464` Emit versioned mem0 JSON and compute cost
- `63693b8f` Implement mem0 Cranelift memory ownership
- `d141f6c5` Implement mem0 C ABI and generated allocation
- `f729f0e5` Implement transactional mem0 C++ code generation
- `da372d80` Add canonical mem0 layout and compute cost analysis
- `41b577a6` Thread typed mem0 option through native backends
- `fc8b5689` Validate mem0 phase zero contracts
- `8e6f0c1b` Plan mem0 memory manager port across native backends

## Working Tree

- Tracked changes at preparation: M9 documentation, journal, plan closeout,
  Rustdoc/CLI wording, and this handoff only.
- Pre-existing untracked user files/directories remain untouched:
  `OSS.md`, `PROMPTS.md`, `Test C++ fad_biquad_spectral_v3/`, `build_all`,
  `fad_use_cases.md`, `push_main.sh`,
  `signal-fir-siggen-completeness-plan-2026-03-12-en.md`,
  `spec_interleave_uz.md`, and `usage-energy-co2-report-2026-08-08.md`.

## Current Goal

- The requested scalar `-mem0` port is complete for generated C, generated
  C++, and native Cranelift, including JSON memory description,
  `compute_cost`, impulse tests, Rustdoc, journal, and staged commits.

## What Changed This Session

- Added typed option propagation and fail-closed capability validation for the
  four mode-zero aliases; `mem1`–`mem3` remain unsupported.
- Added one canonical, checked, target-aware memory layout and corrected scalar
  FIR cost analysis shared by all three backends.
- Implemented transactional generated C++ and strict-C ownership surfaces,
  plus Cranelift pointer-slot lowering and factory/instance/class ownership.
- Added version-2 strict JSON with legacy fields, explicit ABI/layout metadata,
  and deterministic `compute_cost`.
- Added self-contained audited impulse lanes, pinned-C++ differentials,
  Cranelift persistence/rebinding coverage, O0/O3 parity, and sanitizers.

## Decisions / Constraints

- Scope is scalar `mem0` only. Vector custom-memory lowering, `-it`, other
  backends, and `mem1`–`mem3` fail closed.
- Generated C++ preserves the legacy `dsp_memory_manager` names but fixes the
  pinned reference's lifecycle, clone, owner, failure, alignment, and sentinel
  defects.
- Generated C and Cranelift use the shared, versioned, context-carrying,
  alignment-aware `faust_memory_manager` ABI; these are documented Rust
  extensions.
- Serialized Cranelift factories retain mode/layout inputs but never callback
  pointers, so restored `mem0` factories require a fresh manager binding.
- `compute_cost` version 2 describes the effective scalar loop; branch costs
  use a component-wise upper envelope. Common-subset counts match pinned Faust
  C++, while D6 corrections are explicitly allowlisted.

## Validation Run

- `cargo fmt --all` -> pass.
- `cargo doc -p codegen -p compiler -p cranelift-ffi --no-deps` -> pass.
- `cargo clippy --workspace --all-targets -- -D warnings` -> pass.
- `cargo test --workspace --all-targets` -> pass, including the permitted
  hermetic loopback test.
- `cargo run -p xtask -- golden-check` -> pass.
- `cargo run --release -p xtask -- compile-budget-check` -> pass for five
  scalar/vector codegen and eight normalized front-end cases; no baseline
  update.
- `make -B -C tests/impulse-tests all-mem0` -> pass for C, C++, Cranelift,
  JSON/cost parity, and O0/O3.
- `make -C tests/impulse-tests mem0-sanitize` -> pass with ASan/UBSan on the
  supported macOS toolchain.
- `cargo test -p compiler --test mem0_cpp_differential -- --nocapture` -> all
  three live tests pass against pinned Faust C++ `8eebea429`.

## Open Issues / Blockers

- None within the approved `mem0` scope.
- Broader Cranelift FIR subset completion and native-code serialization remain
  pre-existing backend work, not blockers for strict-lowered `mem0` fixtures.

## Next Steps

1. Let CI repeat the cross-platform workspace/golden gates.
2. Treat any future `mem1`–`mem3` or vector support as a separately planned
   compatibility phase; do not silently map it to `mem0`.

## Useful Commands to Resume

- `make -C tests/impulse-tests all-mem0`
- `make -C tests/impulse-tests mem0-sanitize`
- `cargo test -p compiler --test mem0_cpp_differential -- --nocapture`
- `cargo run --release -p xtask -- compile-budget-check`
