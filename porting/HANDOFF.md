# Session Handoff

Date: 2026-10-11

## Repo State

- Branch: `main-dev`
- HEAD: the commit containing this handoff, parent
  `63f0ee248a4bc7692f0c99a62e2a379315395dac` (integer pow port).
- Current follow-up: correct the pasted expansion-corpus failure. The user
  requested committing the correction with the validation limits below.
  No push was requested.

## Working Tree

- The integer pow changes listed below are included in the parent commit.
- This commit includes `crates/compiler/tests/expand_corpus.rs`,
  `tests/expand/{README.md,dsp/020_library_import.dsp,oracle/020_library_import.dsp}`,
  `porting/journal/2026-10-11.md`, and this handoff.
- New file: `tests/expand/dsp/expand_test_library.lib`. No other tracked or
  non-ignored untracked changes remain after committing.
- Committed implementation changes: `crates/sigtype/src/rules.rs`,
  `crates/normalize/src/{normalform,simplify}.rs`, `crates/fir/src/types.rs`,
  the shared/scalar/vector power emission and prototype assembly under
  `crates/transform/src/signal_fir/`, its table-generator interpreter and tests,
  `crates/codegen/src/backends/{cpp/mod,rust/mod,cranelift/math}.rs`.
- Documentation: compatibility registry DIFF-BEH-020,
  `porting/journal/2026-10-11.md`, the generated public API index (positions
  only; no public-item inventory change), and this handoff.
- New tests: `crates/compiler/tests/integer_pow.rs` and
  `crates/compiler/tests/integer_pow_rust.rs`.
- Ignored local scripts `build_all`, `dummy_fdn`, `push_main.sh` are untouched.

## What Changed / Decisions

Expansion follow-up: replace `stdfaust.lib` in `020_library_import` with a
compact corpus-local library authored for the test. It preserves table
generation, two recursion carriers, and both `import` and scoped `library`
calls without using an installed library. The corpus helper supplies
canonical paths for consistent import identity.
Check the expansion's single imported path as a canonical `PathBuf`, and
assert scoped library metadata. Re-capture through `xtask expand-oracle`,
using the pinned C++ checkout `8eebea4294a44a5260484c750d332781ed9f8ffd`,
Faust 2.84.3, flags `-e <fixture> -o <capture>` (default single/scalar).
Only the import fixture's oracle changes. Production compiler code, public
APIs, compatibility differences and memoization are unchanged.

Previous session:

Port C++ `eb21fcccff1ead7fe286f35614a464bea70a5fe2` (#1340): runtime integer
pow uses double pow/rint before int32 conversion in scalar and checked vector
FIR. Typing/promotion had unconditionally forced pow to Real; that earlier
parity gap is closed too. Positive integer constant folding and small constant
powers retain wrapping int32 arithmetic. Compile-time table generators follow
the same integer semantics. CSE keeps a foreign base evaluated only once.

Single pow/rint calls have distinct FIR names (`powf`/`rintf`) from double
calls, without changing the public math-op inventory or the host DSP APIs.
Cranelift and C++ keep explicit suffixed C symbols at their fixed host precision.
Rust single-precision output now supplies the double rint bridge.

FBC is still uniformly f32 or f64 per factory, as in C++: no mixed-precision
opcode/ABI extension. Double FBC passes at optimizer levels 0/max. Single FBC
still loses a unit for 25^6; the current C++ build has no interpreter backend,
so its matching precision limitation was established from the source.

## Validation Run

- Expansion follow-up: all eight `expand_corpus` tests pass; C++ oracle
  check accepts 33 references with the same two unsupported fixtures.
- Follow-up formatting, workspace clippy, all five static gates, and
  golden-check pass.
- `cargo test --workspace --all-targets --no-fail-fast`, rerun with permission
  to bind the HTTP fixture server, passes every target except
  `cranelift-ffi::output_snapshots` (the same 53 pre-existing mismatches).
  The final scoped-library/canonical-path adjustment also passes a fresh
  eight-test expansion run and workspace clippy.

Previous session:

- Native generated C++: single/double, scalar/vector, `-O2` and
  `-O3 -ffast-math`: exact zero residuals for 250^1, 25^6, (-3)^3; small-power
  wrap and negative runtime exponents covered too.
- Generated Rust: built/run with the repository's FaustDsp host surface,
  single/double, `rustc -C opt-level=0/3`: exact residuals.
- Type/promotion/constant-fold/table-generator tests: pass.
- Formatting, clippy, all five static gates, and golden-check: pass.
- Manual Cranelift/faustprobe at both precisions: exact zero residuals.
- Full workspace: two reference tests fail before this port as well; the
  rerun skipping only those tests passes (3,201 tests). All codegen unit tests
  and both native regression suites pass after the final C++ safeguard too.
- Isolated release cost gate: cannot calibrate (3 ms, required floor 4 ms).
  Supplementary alternating release CLI before/after medians are stable:
  karplus 8.625/8.628 ms, spectral_level 325.153/321.422 ms. These include
  process startup and are not a substitute for the gate.

## Open Issues / Blockers

- The expansion oracle drift is resolved by the corpus-local fixture above.
  Previously the local oscillator phasor change failed even with pre-change
  release binary `19efd550`.
- `cranelift-ffi::output_snapshots` reports 53 stale recordings. All 53
  match the pre-change release faustprobe too (46 exactly; seven after
  removing timing fields). No snapshot refresh is part of this correction.
- The sandbox prevented the fixture HTTP server in `remote_source_baseline`;
  it passed in the authorized rerun with permission to bind that local server.
- A release cost measurement during concurrent test/build load exceeded
  spectral_level's front-end ceiling (255 vs 233.1 units). Three isolated runs
  stop at the calibration floor instead. The versioned baseline is untouched;
  a successful gate on a suitable runner and green CI remain unconfirmed.

## Next Steps

1. Review the committed expansion-fixture correction and its captured
   reference (`cargo run -p xtask -- expand-oracle --check`).
2. Resolve the separate faustprobe recording drift; obtain a successful
   release cost gate on a suitable runner and green CI before publication.
3. Single FBC precision extension is outside this C++ commit's port scope.
