# Session Handoff

Date: 2026-08-13

## Repo State

- Branch: `main-dev`
- Implementation HEAD before the full-corpus correction: `1543b70d`
  (`Document and close mem0 port`); the correction commit contains this updated
  handoff.

Recent implementation commits (most recent first):

- `1543b70d` Document and close mem0 port
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

- Tracked changes at preparation: full-corpus `mem0` impulse integration,
  subcontainer codegen/layout corrections, C soundfile architecture support,
  documentation, journal, and this handoff.
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
- Added full-corpus audited impulse lanes, a separate self-contained smoke
  audit, pinned-C++ differentials, Cranelift persistence/rebinding coverage,
  O0/O3 parity, and sanitizers. The full corpus found and now guards C/C++
  subcontainer layout/lifetime defects and the strict-C soundfile contract.

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
- `cargo run --release -p xtask -- compile-budget-check` -> three runs each
  reproduce only the pre-existing noisy `reverb_designer` vector overage (about
  1,022 normalized units against 953.982 twice, then 5,505 ms against the 5,411
  ms absolute/ratio allowance); all other cases pass and the baseline remains
  unchanged.
- `make -j8 -C tests/impulse-tests all-mem0` -> pass for all 94
  oracle-supported `dsp/` inputs on C, C++, and Cranelift over 15,000 frames,
  per-DSP JSON/cost parity, and the focused O0/O3 audit; 39 inputs are
  explicitly classified as oracle-unsupported.
- `make -B -j8 -C tests/impulse-tests -f Make.mem0 all
  mem0_local_reference=1 MEM0_ROOT=build/mem0-full-local
  MEM0_IR=ir/mem0-full-local` -> pass after regenerating both the ordinary
  faust-rs C++ references and every managed artifact for all 94 inputs.
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
